//! Typed Laya choices through an individually claimed LARM provider.
//! Discovery does not change the user's conversation model or saved profile.
use futures_util::StreamExt;
use saaa_larm_session::{ProfilePreference, ProfileVariant, Session};
use serde_json::{json, Value};
use std::time::Duration;

// Unit tests and avatar decisions share one lane: overlapping claims/releases
// on the same LARM provider can invalidate each other's allocation credential.
static DECISION_LANE: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

pub(crate) struct Decision {
    pub model: String,
    pub response: Value,
}

pub(crate) fn test_question() -> Value {
    json!({"action":{"type":"choice","instructions":"指示に合った動作を1つ選んでください。",
        "criteria":{"left":"左に移動する","right":"右に移動する","jump":"ジャンプする","stop":"停止する"}}})
}

pub(crate) fn avatar_question() -> Value {
    serde_json::from_str(include_str!("laya/avatar-question.json"))
        .expect("checked-in Laya question JSON")
}

pub(crate) fn speech_question() -> Value {
    let mut question = avatar_question();
    question["voice"] = json!({"type":"choice",
        "instructions":"この文章をアシスタントが読み上げる際の声の調子を1つ選んでください。",
        "criteria":{"natural":"普段通りの落ち着いた説明", "bright":"明るく親しみのある挨拶や喜び",
        "gentle":"悲しみへの共感、安心させる優しい声", "serious":"重要な注意や真剣な説明",
        "excited":"大きな成功や驚きを強く喜ぶ声"}});
    question
}

pub(crate) async fn choose(
    harness: &crate::HarnessSettings,
    credential: &str,
    state: &Value,
    questions: Value,
    on_progress: &(dyn Fn(&str) + Send + Sync),
) -> Result<Decision, String> {
    let _lane = tokio::time::timeout(Duration::from_secs(20), DECISION_LANE.lock())
        .await
        .map_err(|_| "Layaの判断が混雑しています。少し待って再実行してください。")?;
    if state.is_null()
        || state.as_str().is_some_and(|s| s.trim().is_empty())
        || state.to_string().len() > 4096
    {
        return Err("Layaへの入力は1〜4096バイトにしてください。".into());
    }
    let client = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(15))
        .build()
        .map_err(|_| "Layaの通信を準備できませんでした。")?;
    let base = url::Url::parse(&harness.address).map_err(|_| "LARMの接続先が不正です。")?;
    if !saaa_larm_session::local_url(&base) || base.path() != "/" {
        return Err("LARMの接続先が不正です。".into());
    }
    on_progress("model_preparing");
    let preferred = super::larm_resources::profile::preference(harness.larm_profile.as_deref());
    let mut variants = vec![preferred];
    for variant in ProfileVariant::ALL {
        let next = ProfilePreference::Variant(variant);
        if !variants.contains(&next) {
            variants.push(next);
        }
    }
    let mut selected = None;
    for preference in variants {
        let selector = match &preference {
            ProfilePreference::Variant(v) => v.selector(),
            ProfilePreference::Explicit(v) => v,
        };
        let catalog = saaa_larm_session::catalog::fetch(&client, &base, credential, selector)
            .await
            .map_err(|code| format!("Layaの一覧を取得できませんでした: {code}"))?;
        if let Some(provider) = catalog
            .providers
            .iter()
            .find(|p| p.protocol == "larm.system-one.v1" && p.model.starts_with("laya"))
        {
            let name = match provider.name.as_str() {
                "system-one" => "system-one",
                "systemone" => "systemone",
                _ => return Err("LayaのProvider名に対応していません。".into()),
            };
            selected = Some((preference, name));
            break;
        }
    }
    let (preference, name) =
        selected.ok_or("LARMにLayaがありません。接続先のProvider構成を確認してください。")?;
    let (_stop, cancel) = tokio::sync::watch::channel(false);
    let session = match Session::connect_with_profile_credential_key_phase_and_providers(
        &harness.address,
        preference,
        credential.into(),
        format!("saaa-laya-{}", uuid::Uuid::new_v4().simple()),
        cancel,
        None,
        Some(vec![name]),
    )
    .await
    {
        Ok(session) => session,
        Err(error) => {
            let code = error.code;
            if let Some(cleanup) = error.cleanup {
                let _ = cleanup.close().await;
            }
            return Err(format!("Layaの接続を準備できませんでした: {code}"));
        }
    };
    on_progress("provider_request");
    let result = tokio::time::timeout(Duration::from_secs(15), async {
        let lease = session.acquire(name).await.map_err(str::to_string)?;
        let provider = lease.provider();
        if provider.protocol != "larm.system-one.v1" || !provider.model.starts_with("laya") {
            return Err("LayaのProvider契約が一致しません。".to_string());
        }
        let response = client
            .post(provider.endpoint("systemone").map_err(str::to_string)?)
            .bearer_auth(provider.token())
            .timeout(
                lease
                    .request_budget(Duration::from_secs(12))
                    .map_err(str::to_string)?,
            )
            .json(&json!({"model":provider.model,"state":state,"questions":questions}))
            .send()
            .await
            .map_err(|_| "Layaへの接続に失敗しました。")?;
        if !response.status().is_success() {
            return Err(format!(
                "Layaが要求を受け付けませんでした（HTTP {}）。",
                response.status().as_u16()
            ));
        }
        let mut bytes = Vec::new();
        let mut stream = response.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|_| "Layaの結果の取得が中断されました。")?;
            if bytes.len() + chunk.len() > 65_536 {
                return Err("Layaの結果が上限を超えました。".into());
            }
            bytes.extend_from_slice(&chunk);
        }
        let response: Value =
            serde_json::from_slice(&bytes).map_err(|_| "Layaの結果がJSONではありません。")?;
        if response["model"] != provider.model {
            return Err("Layaの応答モデルが一致しません。".into());
        }
        validate_answer(&response, &questions)?;
        Ok(Decision {
            model: provider.model.clone(),
            response,
        })
    })
    .await
    .map_err(|_| "Layaの判断が時間内に完了しませんでした。".to_string())
    .and_then(|r| r);
    on_progress("releasing");
    session
        .close()
        .await
        .map_err(|_| "Layaの接続の解放を確認できませんでした。")?;
    result
}

fn validate_answer(response: &Value, questions: &Value) -> Result<(), String> {
    if response
        .pointer("/usage/truncated")
        .and_then(Value::as_bool)
        == Some(true)
        || response
            .pointer("/usage/state_tokens_dropped")
            .and_then(Value::as_u64)
            .unwrap_or(0)
            > 0
    {
        return Err("Layaの入力が切り詰められました。短い文で再実行してください。".into());
    }
    for (key, question) in questions.as_object().ok_or("Layaの質問が不正です。")? {
        let answer = &response["answers"][key];
        if answer["type"] != "choice"
            || !answer["confidence"]
                .as_f64()
                .is_some_and(|n| (0.0..=1.0).contains(&n))
        {
            return Err("Layaの選択結果の形式が不正です。".into());
        }
        let choice = response["answers"][key]["choice"]
            .as_str()
            .ok_or("Layaから選択結果が返りませんでした。")?;
        if question["criteria"].get(choice).is_none() {
            return Err("Layaが選択肢にない結果を返しました。".into());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_missing_unknown_and_truncated_choices() {
        let question = test_question();
        assert!(validate_answer(
            &json!({"answers":{"action":{"type":"choice","choice":"right","confidence":0.8}}}),
            &question
        )
        .is_ok());
        for value in [
            json!({}),
            json!({"answers":{"action":{"type":"choice","choice":"run","confidence":0.8}}}),
            json!({"answers":{"action":{"type":"choice","choice":"right","confidence":0.8}},"usage":{"truncated":true}}),
        ] {
            assert!(validate_answer(&value, &question).is_err());
        }
    }
}
