use rusqlite::Connection;
use serde::Serialize;
use serde_json::Value;
use sha2::{Digest, Sha256};

#[derive(Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TtsSelection {
    System { voice: String },
    Larm { voice: String },
}

#[derive(Clone)]
pub struct PreviewConfig {
    pub harness_address: String,
    pub profile: saaa_larm_session::ProfilePreference,
    pub profile_label: String,
    pub tts: TtsSelection,
    pub fingerprint: String,
}

pub fn load(db: &Connection) -> Result<PreviewConfig, String> {
    let model_raw: String = db
        .query_row(
            "SELECT value_json FROM settings_documents WHERE namespace='providers.model' AND key='default'",
            [],
            |row| row.get(0),
        )
        .map_err(|_| "保存済みProvider設定を読み取れません。".to_string())?;
    let route_raw: String = db
        .query_row(
            "SELECT value_json FROM settings_documents WHERE namespace='routing.tasks' AND key='default'",
            [],
            |row| row.get(0),
        )
        .map_err(|_| "保存済み音声ルートを読み取れません。".to_string())?;
    let model: Value = serde_json::from_str(&model_raw)
        .map_err(|_| "Provider設定の形式が正しくありません。".to_string())?;
    let route: Value = serde_json::from_str(&route_raw)
        .map_err(|_| "音声ルートの形式が正しくありません。".to_string())?;
    let harness = &model["harness"];
    let address = harness["address"]
        .as_str()
        .filter(|value| !value.is_empty())
        .ok_or("LARMの接続先が設定されていません。")?
        .to_string();
    let profile_label = harness["larmProfile"]
        .as_str()
        .unwrap_or(saaa_larm_session::DEFAULT_SELECTOR)
        .to_string();
    let variant = saaa_larm_session::ProfileVariant::from_selector(&profile_label)
        .or_else(|| {
            saaa_larm_session::LEGACY_PROFILE_IDS
                .contains(&profile_label.as_str())
                .then_some(saaa_larm_session::ProfileVariant::Conversation)
        })
        .ok_or("選択済みLARM profileには対応していません。")?;
    let route = &route["voiceSpeak"];
    let tts = match (route["source"].as_str(), route["providerId"].as_str()) {
        (Some("harness"), _) => TtsSelection::Larm {
            voice: harness["ttsVoice"]
                .as_str()
                .filter(|value| !value.is_empty())
                .ok_or("LARM TTSのvoiceが選択されていません。")?
                .to_string(),
        },
        (Some("provider"), Some(id)) => {
            let providers = model["providers"]
                .as_array()
                .ok_or("Provider一覧の形式が正しくありません。")?;
            let selected = providers
                .iter()
                .find(|provider| provider["id"] == id && provider["enabled"] == true)
                .ok_or("選択済みTTS Providerが見つかりません。")?;
            match selected["kind"].as_str() {
                Some("system-tts") => TtsSelection::System {
                    voice: selected["voice"]
                        .as_str()
                        .ok_or("System TTSのvoiceがありません。")?
                        .to_string(),
                },
                _ => return Err("選択済みTTS方式はpreview未対応です。".into()),
            }
        }
        _ => return Err("音声ルートが未設定です。".into()),
    };
    let mut hasher = Sha256::new();
    hasher.update(model_raw.as_bytes());
    hasher.update(route_raw.as_bytes());
    let mut credential_refs = db
        .prepare("SELECT service,account FROM credential_secrets ORDER BY service,account")
        .map_err(|_| "資格情報参照を確認できません。".to_string())?;
    let refs = credential_refs
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(|_| "資格情報参照を確認できません。".to_string())?;
    for item in refs {
        let (service, account) = item.map_err(|_| "資格情報参照を確認できません。")?;
        hasher.update(service.as_bytes());
        hasher.update([0]);
        hasher.update(account.as_bytes());
        hasher.update([0]);
    }
    Ok(PreviewConfig {
        harness_address: address,
        profile: saaa_larm_session::ProfilePreference::Variant(variant),
        profile_label,
        tts,
        fingerprint: format!("{:x}", hasher.finalize()),
    })
}
