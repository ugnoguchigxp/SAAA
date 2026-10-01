#[cfg(test)]
use rusqlite::params;
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};
use tauri::Emitter;

mod contracts;
#[path = "tts_dictionary/custom_matcher.rs"]
mod custom_matcher;
mod proposals;
pub(crate) mod service;
pub(crate) mod tools;
pub(crate) use contracts::{validate, Entry, ExpectedEntry};
pub(crate) use custom_matcher::CompiledDictionary;

use crate::{now_iso, AppState, ModelProviderSettings, RunCancellation};

#[derive(Default)]
pub(crate) struct DictionaryCache {
    inner: Mutex<CacheState>,
    pub(crate) proposals: proposals::Proposals,
}

#[derive(Default)]
struct CacheState {
    revision: u64,
    snapshot: Option<Arc<CompiledDictionary>>,
}

impl DictionaryCache {
    pub(crate) fn snapshot(
        &self,
        readers: &crate::persistence::SqliteReaders,
    ) -> Result<Arc<CompiledDictionary>, String> {
        let revision = {
            let state = self
                .inner
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if let Some(snapshot) = &state.snapshot {
                return Ok(snapshot.clone());
            }
            state.revision
        };
        let loaded = Arc::new(compile(readers.read(list)?));
        let mut state = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(snapshot) = &state.snapshot {
            return Ok(snapshot.clone());
        }
        if state.revision == revision {
            state.snapshot = Some(loaded.clone());
        }
        Ok(loaded)
    }

    #[cfg(test)]
    fn publish(&self, entries: Vec<Entry>) {
        self.publish_compiled(compile(entries));
    }

    pub(crate) fn publish_compiled(&self, dictionary: CompiledDictionary) {
        let snapshot = Arc::new(dictionary);
        let mut state = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.revision = state.revision.wrapping_add(1);
        state.snapshot = Some(snapshot);
    }
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
    expected: ExpectedEntry,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct PreviewInput {
    text: String,
    original: Option<String>,
    entry: Option<Entry>,
    #[serde(default)]
    raw: bool,
}

pub(crate) fn list(connection: &Connection) -> Result<Vec<Entry>, String> {
    service::list(connection)
}

pub(crate) fn apply(text: &str, entries: &[Entry]) -> String {
    compile(entries.to_vec()).apply(text)
}

pub(crate) fn apply_saved(connection: &Connection, text: &str) -> Result<String, String> {
    Ok(compile(list(connection)?).apply(text))
}

pub(crate) fn compile(entries: Vec<Entry>) -> CompiledDictionary {
    CompiledDictionary::new(
        entries
            .into_iter()
            .map(|entry| (entry.written, entry.spoken))
            .collect(),
    )
}

#[tauri::command]
pub(crate) fn list_tts_dictionary(state: tauri::State<'_, AppState>) -> Result<Vec<Entry>, String> {
    state.sqlite_readers.read(list)
}

#[tauri::command]
pub(crate) fn search_tts_dictionary_presets(
    _state: tauri::State<'_, AppState>,
    query: String,
) -> Result<PresetSearchResult, String> {
    if query.len() > 200 || query.chars().any(char::is_control) {
        return Err("検索語が長すぎるか、不正です。".into());
    }
    Ok(PresetSearchResult {
        total: 0,
        entries: Vec::new(),
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
    let custom = state
        .sqlite_readers
        .read(|connection| service::lookup(connection, &written))?;
    if let Some(spoken) = custom {
        return Ok(Some(ExistingEntry {
            source: "custom",
            entry: Entry { written, spoken },
        }));
    }
    Ok(None)
}

#[tauri::command]
pub(crate) fn save_tts_dictionary_entry(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    input: SaveInput,
) -> Result<Vec<Entry>, String> {
    let (entries, changed) = state.sqlite_writer.write(|connection| {
        let mutation = service::save(
            connection,
            input.original.as_deref(),
            &input.entry,
            &input.expected,
            &now_iso(),
            |_| Ok(()),
        )?;
        let changed = mutation.dictionary.is_some();
        if let Some(dictionary) = mutation.dictionary {
            state.tts_dictionary_cache.publish_compiled(dictionary);
        }
        Ok((mutation.entries, changed))
    })?;
    if changed {
        let _ = app.emit("tts-dictionary-changed", ());
    }
    Ok(entries)
}

#[tauri::command]
pub(crate) fn delete_tts_dictionary_entry(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    written: String,
    expected: ExpectedEntry,
) -> Result<Vec<Entry>, String> {
    let (entries, changed) = state.sqlite_writer.write(|connection| {
        let mutation = service::delete(connection, &written, &expected)?;
        let changed = mutation.dictionary.is_some();
        if let Some(dictionary) = mutation.dictionary {
            state.tts_dictionary_cache.publish_compiled(dictionary);
        }
        Ok((mutation.entries, changed))
    })?;
    if changed {
        let _ = app.emit("tts-dictionary-changed", ());
    }
    Ok(entries)
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
    let text = if input.raw {
        input.text.trim().to_string()
    } else {
        apply(&crate::voice_text::text_for_speech(&input.text), &entries)
    };
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
    fn only_custom_readings_are_applied() {
        assert_eq!(apply("今日", &[]), "今日");
        assert_eq!(apply("銀行", &[]), "銀行");
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
    fn cached_snapshot_changes_only_when_published() {
        let connection = Connection::open_in_memory().expect("memory database");
        crate::persistence::schema::initialize_database(&connection).expect("schema");
        let writer = Arc::new(crate::persistence::SqliteWriter::from_connection(
            connection,
        ));
        let readers = crate::persistence::SqliteReaders::serialized(writer);
        let cache = DictionaryCache::default();
        let first = cache.snapshot(&readers).expect("initial snapshot");
        assert_eq!(first.apply("今日"), "今日");
        cache.publish(vec![Entry {
            written: "今日".into(),
            spoken: "きょう".into(),
        }]);
        assert_eq!(first.apply("今日"), "今日");
        assert_eq!(
            cache
                .snapshot(&readers)
                .expect("updated snapshot")
                .apply("今日"),
            "きょう"
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
