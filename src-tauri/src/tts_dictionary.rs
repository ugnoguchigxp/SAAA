use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[path = "tts_dictionary/preset_matcher.rs"]
mod preset_matcher;

use crate::{database_error, now_iso, AppState, ModelProviderSettings, RunCancellation};

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Entry {
    pub(crate) written: String,
    pub(crate) spoken: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PresetSearchResult {
    total: usize,
    entries: Vec<Entry>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ExistingEntry {
    source: &'static str,
    entry: Entry,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct SaveInput {
    original: Option<String>,
    preset_override: Option<String>,
    entry: Entry,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct PreviewInput {
    text: String,
    original: Option<String>,
    entry: Option<Entry>,
}

fn validate(entry: &Entry) -> Result<(), String> {
    if entry.written.trim() != entry.written
        || entry.spoken.trim() != entry.spoken
        || entry.written.is_empty()
        || entry.written.len() > 200
        || entry.spoken.len() > 400
        || entry.written.chars().any(char::is_control)
        || entry.spoken.chars().any(char::is_control)
    {
        return Err("表記または読み方が不正です。".into());
    }
    Ok(())
}

pub(crate) fn list(connection: &Connection) -> Result<Vec<Entry>, String> {
    let mut statement = connection
        .prepare("SELECT written, spoken FROM tts_dictionary ORDER BY written")
        .map_err(database_error)?;
    let result = statement
        .query_map([], |row| {
            Ok(Entry {
                written: row.get(0)?,
                spoken: row.get(1)?,
            })
        })
        .map_err(database_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(database_error);
    result
}

pub(crate) fn apply(text: &str, entries: &[Entry]) -> String {
    let custom: Vec<_> = entries
        .iter()
        .map(|entry| (entry.written.clone(), entry.spoken.clone()))
        .collect();
    preset_matcher::apply(text, &custom)
}

pub(crate) fn apply_saved(connection: &Connection, text: &str) -> Result<String, String> {
    Ok(apply(text, &list(connection)?))
}

pub(crate) fn ready_stream_prefix_len(text: &str, entries: &[Entry]) -> usize {
    let custom: Vec<_> = entries
        .iter()
        .map(|entry| (entry.written.clone(), entry.spoken.clone()))
        .collect();
    preset_matcher::ready_prefix_len(text, &custom)
}

#[tauri::command]
pub(crate) fn list_tts_dictionary(state: tauri::State<'_, AppState>) -> Result<Vec<Entry>, String> {
    state.sqlite_readers.read(list)
}

#[tauri::command]
pub(crate) fn search_tts_dictionary_presets(
    state: tauri::State<'_, AppState>,
    query: String,
) -> Result<PresetSearchResult, String> {
    if query.len() > 200 || query.chars().any(char::is_control) {
        return Err("検索語が長すぎるか、不正です。".into());
    }
    let excluded: std::collections::HashSet<String> = state
        .sqlite_readers
        .read(list)?
        .into_iter()
        .map(|entry| entry.written)
        .collect();
    let overridden = excluded
        .iter()
        .filter(|written| preset_matcher::lookup(written).is_some())
        .count();
    Ok(PresetSearchResult {
        total: preset_matcher::count() - overridden,
        entries: preset_matcher::search(&query, 100, &excluded)
            .into_iter()
            .map(|(written, spoken)| Entry { written, spoken })
            .collect(),
    })
}

#[tauri::command]
pub(crate) fn lookup_tts_dictionary_entry(
    state: tauri::State<'_, AppState>,
    written: String,
) -> Result<Option<ExistingEntry>, String> {
    if written.len() > 200 || written.chars().any(char::is_control) {
        return Err("表記が長すぎるか、不正です。".into());
    }
    let custom = state.sqlite_readers.read(|connection| {
        let mut statement = connection
            .prepare("SELECT spoken FROM tts_dictionary WHERE written=?1")
            .map_err(database_error)?;
        let mut rows = statement.query(params![written]).map_err(database_error)?;
        rows.next()
            .map_err(database_error)?
            .map(|row| row.get::<_, String>(0).map_err(database_error))
            .transpose()
    })?;
    if let Some(spoken) = custom {
        return Ok(Some(ExistingEntry {
            source: "custom",
            entry: Entry { written, spoken },
        }));
    }
    Ok(
        preset_matcher::lookup(&written).map(|spoken| ExistingEntry {
            source: "preset",
            entry: Entry { written, spoken },
        }),
    )
}

#[tauri::command]
pub(crate) fn save_tts_dictionary_entry(
    state: tauri::State<'_, AppState>,
    input: SaveInput,
) -> Result<Vec<Entry>, String> {
    validate(&input.entry)?;
    if preset_matcher::lookup(&input.entry.written).is_some()
        && input.original.as_deref() != Some(input.entry.written.as_str())
        && input.preset_override.as_deref() != Some(input.entry.written.as_str())
    {
        return Err(
            "その表記は既定語彙に登録されています。既定語彙から選択して編集してください。".into(),
        );
    }
    state.sqlite_writer.write(|connection| {
        let transaction = connection.transaction().map_err(database_error)?;
        if input.original.as_deref() != Some(input.entry.written.as_str()) {
            let existing: bool = transaction.query_row(
                "SELECT EXISTS(SELECT 1 FROM tts_dictionary WHERE written=?1)",
                params![input.entry.written], |row| row.get(0),
            ).map_err(database_error)?;
            if existing { return Err("その表記はすでに登録されています。".into()); }
        }
        if let Some(original) = input.original.as_deref() {
            if original != input.entry.written {
                transaction.execute("DELETE FROM tts_dictionary WHERE written=?1", params![original]).map_err(database_error)?;
            }
        }
        transaction.execute(
            "INSERT INTO tts_dictionary(written, spoken, updated_at) VALUES(?1, ?2, ?3)
             ON CONFLICT(written) DO UPDATE SET spoken=excluded.spoken, updated_at=excluded.updated_at",
            params![input.entry.written, input.entry.spoken, now_iso()],
        ).map_err(database_error)?;
        transaction.commit().map_err(database_error)?;
        list(connection)
    })
}

#[tauri::command]
pub(crate) fn delete_tts_dictionary_entry(
    state: tauri::State<'_, AppState>,
    written: String,
) -> Result<Vec<Entry>, String> {
    state.sqlite_writer.write(|connection| {
        connection
            .execute(
                "DELETE FROM tts_dictionary WHERE written=?1",
                params![written],
            )
            .map_err(database_error)?;
        list(connection)
    })
}

#[tauri::command]
pub(crate) async fn preview_tts_dictionary(
    state: tauri::State<'_, AppState>,
    input: PreviewInput,
) -> Result<(), String> {
    if input.text.trim().is_empty() || input.text.len() > 1000 {
        return Err("試し読みの文は1〜1000バイトにしてください。".into());
    }
    if let Some(entry) = &input.entry {
        validate(entry)?;
    }
    let (entries, providers, route) = state.sqlite_readers.read(|connection| {
        Ok((
            list(connection)?,
            crate::persistence::load_model_providers(connection)?,
            crate::persistence::load_routing_settings(connection)?.voice_speak,
        ))
    })?;
    let mut entries = entries;
    if let Some(entry) = input.entry {
        entries.retain(|existing| {
            existing.written != entry.written
                && input.original.as_deref() != Some(existing.written.as_str())
        });
        entries.push(entry);
    }
    let text = apply(&crate::voice_text::text_for_speech(&input.text), &entries);
    if text.is_empty() {
        return Err("読み上げ可能な文がありません。".into());
    }
    let cancellation = Arc::new(RunCancellation::default());
    let (bytes, format) = if route.source == "harness" {
        (
            crate::runtime::provider_unit_test::preview_harness_tts(&providers, &text).await?,
            "wav".to_string(),
        )
    } else {
        let provider = providers
            .providers
            .iter()
            .find(|provider| {
                route.provider_id.as_deref() == Some(provider.id()) && provider.enabled()
            })
            .ok_or("設定済みのTTS Providerが見つかりません。")?;
        match provider {
            ModelProviderSettings::CloudTts(provider) => (
                crate::voice::cloud_tts::synthesize(
                    provider,
                    &text,
                    route.timeout_ms.min(120_000),
                    cancellation.clone(),
                )
                .await?
                .to_vec(),
                "wav".to_string(),
            ),
            ModelProviderSettings::SystemTts(provider) => {
                let directory =
                    tempfile::tempdir().map_err(|_| "TTS一時領域を作成できませんでした。")?;
                let path = crate::voice::system_tts::render_tts_artifact(
                    text,
                    provider.voice.clone(),
                    directory.path().to_path_buf(),
                    cancellation.clone(),
                )
                .await?;
                (
                    std::fs::read(path)
                        .map_err(|_| "生成した音声を読み込めませんでした。".to_string())?,
                    "wav".to_string(),
                )
            }
            _ => return Err("選択したProviderはTTSではありません。".into()),
        }
    };
    let mut decoder = crate::voice::http_audio::decode::Decoder::new(&format)?;
    let samples = decoder.push(&bytes)?;
    decoder.finish()?;
    let audio_format = decoder.format.ok_or("TTS音声の形式がありません。")?;
    let packet_samples = (audio_format.rate as usize / 20) * audio_format.channels as usize;
    let player =
        crate::voice::local_audio_output::ContinuousPlayback::start(cancellation.clone(), || {});
    for packet in samples.chunks(packet_samples.max(1)) {
        player
            .sender()
            .send((audio_format, packet.to_vec()))
            .await
            .map_err(|_| "音声を再生できませんでした。".to_string())?;
    }
    player.finish().await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn longest_match_and_no_recursive_replacement() {
        let entries = vec![
            Entry {
                written: "AI".into(),
                spoken: "エーアイ".into(),
            },
            Entry {
                written: "OpenAI".into(),
                spoken: "オープンAI".into(),
            },
        ];
        assert_eq!(apply("OpenAIとAI", &entries), "オープンAIとエーアイ");
    }

    #[test]
    fn empty_reading_removes_symbol_without_changing_other_text() {
        let entries = vec![Entry {
            written: "■".into(),
            spoken: String::new(),
        }];
        assert!(validate(&entries[0]).is_ok());
        assert_eq!(apply("確認■しました。", &entries), "確認しました。");
        assert_eq!(apply("■", &entries), "");
    }

    #[test]
    fn preset_reading_is_used_and_custom_entry_overrides_it() {
        assert_eq!(apply("銀行", &[]), "ギンコウ");
        let custom = vec![Entry {
            written: "銀行".into(),
            spoken: "バンク".into(),
        }];
        assert_eq!(apply("銀行", &custom), "バンク");
        assert_eq!(apply("明日", &[]), "明日");
    }

    #[test]
    fn persisted_entries_are_loaded_from_isolated_database() {
        let connection = Connection::open_in_memory().expect("memory database");
        crate::persistence::schema::initialize_database(&connection).expect("schema");
        connection
            .execute(
                "INSERT INTO tts_dictionary(written, spoken, updated_at) VALUES(?1, ?2, ?3)",
                params!["重複", "ちょうふく", now_iso()],
            )
            .expect("entry saved");
        assert_eq!(
            apply_saved(&connection, "重複を確認"),
            Ok("ちょうふくを確認".into())
        );
    }

    #[test]
    fn rejects_empty_and_control_character_entries() {
        assert!(validate(&Entry {
            written: " ".into(),
            spoken: "よみ".into()
        })
        .is_err());
        assert!(validate(&Entry {
            written: "語".into(),
            spoken: "よみ\n".into()
        })
        .is_err());
    }
}
