//! Purpose settings prepared without changing persisted legacy providers or capture ownership.
use super::*;
use crate::providers::service_registry::{AdapterKind, Purpose, ResolvedRoute};

#[derive(Clone)]
pub(super) struct PreparedVoice {
    pub(super) providers: crate::ModelProvidersSettings,
    pub(super) route: crate::VoiceRouteSettings,
    pub(super) resolved: ResolvedRoute,
    pub(super) deadline: tokio::time::Instant,
}

pub(super) fn prepare(
    db: &rusqlite::Connection,
    purpose: Purpose,
) -> Result<PreparedVoice, String> {
    let loaded = persistence::service_registry_store::load_registry(db)?;
    let resolved = crate::providers::service_registry::resolve_route(&loaded.snapshot, purpose)
        .map_err(|e| format!("音声サービスの設定を確認してください: {e:?}"))?;
    let mut providers = persistence::load_model_providers(db)?;
    let resource = loaded
        .snapshot
        .resource(&resolved.resource_id)
        .ok_or("音声モデルがありません")?;
    let custom = resolved.resource_id.starts_with("res:svc-");
    let provider_id = if resolved.connection_id == "conn:harness" {
        None
    } else if custom {
        let id = format!("registry:{}", resolved.connection_id);
        let connection = loaded
            .snapshot
            .connection(&resolved.connection_id)
            .ok_or("音声サービスがありません")?;
        let provider = match resolved.adapter_kind {
            AdapterKind::HttpAsr => {
                ModelProviderSettings::CloudAsr(crate::CloudAsrProviderSettings {
                    id: id.clone(),
                    enabled: true,
                    label: connection.label.clone(),
                    location: connection.location.clone(),
                    endpoint: resolved.endpoint.clone(),
                    model: resolved.model.clone(),
                    language: resource.detail.clone().unwrap_or_else(|| "auto".into()),
                    authentication: connection.authentication.clone(),
                    transport: "http".into(),
                })
            }
            AdapterKind::HttpTts => {
                ModelProviderSettings::CloudTts(crate::CloudTtsProviderSettings {
                    id: id.clone(),
                    enabled: true,
                    label: connection.label.clone(),
                    location: connection.location.clone(),
                    endpoint: resolved.endpoint.clone(),
                    model: resolved.model.clone(),
                    voice: resource.detail.clone().unwrap_or_default(),
                    response_format: "wav".into(),
                    authentication: connection.authentication.clone(),
                    style: None,
                    speed: None,
                    pitch_scale: None,
                    intonation_scale: None,
                })
            }
            AdapterKind::SystemTts => {
                ModelProviderSettings::SystemTts(crate::SystemTtsProviderSettings {
                    id: id.clone(),
                    enabled: true,
                    label: connection.label.clone(),
                    location: "local".into(),
                    voice: resource.detail.clone().unwrap_or_default(),
                })
            }
            _ => return Err("この音声接続方式は未対応です".into()),
        };
        providers.providers.push(provider);
        Some(id)
    } else {
        Some(
            resolved
                .resource_id
                .strip_prefix("res:")
                .ok_or("音声モデルIDが不正です")?
                .to_string(),
        )
    };
    let route = crate::VoiceRouteSettings {
        source: if provider_id.is_some() {
            "provider"
        } else {
            "harness"
        }
        .into(),
        provider_id,
        fallback_provider_ids: Vec::new(),
        timeout_ms: resolved.timeout_ms,
        attempt_timeout_ms: resolved.attempt_timeout_ms,
    };
    let deadline =
        tokio::time::Instant::now() + std::time::Duration::from_millis(resolved.timeout_ms);
    Ok(PreparedVoice {
        providers,
        route,
        resolved,
        deadline,
    })
}

impl PreparedVoice {
    pub(super) fn validate(&self, state: &AppState) -> Result<u64, String> {
        state
            .sqlite_readers
            .read(|db| direct_route::validate_route(db, &self.resolved))?;
        let remaining = self
            .deadline
            .saturating_duration_since(tokio::time::Instant::now())
            .as_millis() as u64;
        if remaining == 0 {
            return Err("この発話の全体期限を超えました".into());
        }
        Ok(remaining
            .min(self.resolved.attempt_timeout_ms.unwrap_or(remaining))
            .min(120_000))
    }
}

struct Utterance {
    owner: usize,
    id: String,
    prepared: PreparedVoice,
    closed: bool,
}
static UTTERANCES: OnceLock<std::sync::Mutex<std::collections::VecDeque<Utterance>>> =
    OnceLock::new();

pub(super) fn utterance(state: &AppState, id: &str) -> Result<PreparedVoice, String> {
    let mut rows = UTTERANCES
        .get_or_init(Default::default)
        .lock()
        .map_err(|_| "発話の設定を取得できません")?;
    let owner = Arc::as_ptr(&state.sqlite_writer) as usize;
    if let Some(row) = rows.iter().find(|r| r.owner == owner && r.id == id) {
        if row.closed {
            return Err("この発話は確定済みです".into());
        }
        return Ok(row.prepared.clone());
    }
    // Do not evict a live utterance and silently pin it again to new settings.
    while rows.len() >= 128 {
        let Some(index) = rows
            .iter()
            .position(|r| r.closed || r.prepared.deadline <= tokio::time::Instant::now())
        else {
            return Err("発話の処理が混雑しています".into());
        };
        rows.remove(index);
    }
    let prepared = state
        .sqlite_readers
        .read(|db| prepare(db, Purpose::VoiceTranscribe))?;
    state.sqlite_writer.write(|db| {
        db.execute_batch("CREATE TABLE IF NOT EXISTS purpose_voice_utterances(id TEXT PRIMARY KEY,state TEXT NOT NULL)").map_err(database_error)?;
        let inserted=db.execute("INSERT OR IGNORE INTO purpose_voice_utterances(id,state) VALUES(?1,'open')",[id]).map_err(database_error)?;
        if inserted!=1 {return Err("この発話は既に開始済みか確定済みです。再送で接続先を変更できません".into());}Ok(())
    })?;
    rows.push_back(Utterance {
        owner,
        id: id.into(),
        prepared: prepared.clone(),
        closed: false,
    });
    Ok(prepared)
}

pub(super) fn close_utterance(state: &AppState, id: &str) -> Result<(), String> {
    state.sqlite_writer.write(|db| {
        db.execute(
            "UPDATE purpose_voice_utterances SET state='closed' WHERE id=?1",
            [id],
        )
        .map_err(database_error)?;
        Ok(())
    })?;
    let owner = Arc::as_ptr(&state.sqlite_writer) as usize;
    if let Some(mut rows) = UTTERANCES.get().and_then(|s| s.lock().ok()) {
        if let Some(row) = rows.iter_mut().find(|r| r.owner == owner && r.id == id) {
            row.closed = true;
        }
    }
    Ok(())
}

#[cfg(feature = "conversation-queue-e2e")]
pub(super) fn reset_fixture() {
    if let Some(mut rows) = UTTERANCES.get().and_then(|s| s.lock().ok()) {
        rows.clear();
    }
}

#[cfg(all(test, feature = "conversation-queue-e2e"))]
mod tests {
    use super::*;
    use crate::providers::service_registry::{
        BindingReview, Capability, ServiceConnection, ServiceResource,
    };
    fn database(endpoint: &str) -> rusqlite::Connection {
        let mut db = rusqlite::Connection::open_in_memory().unwrap();
        crate::initialize_database(&db).unwrap();
        let loaded = persistence::service_registry_store::load_registry(&db).unwrap();
        let mut snapshot = loaded.snapshot;
        snapshot.connections.push(ServiceConnection {
            connection_id: "conn:svc-voice-fixture".into(),
            label: "ASR fixture".into(),
            adapter_kind: AdapterKind::HttpAsr,
            endpoint: endpoint.into(),
            location: "cloud".into(),
            authentication: "none".into(),
            credential_ref: None,
            enabled: true,
        });
        snapshot.resources.push(ServiceResource {
            resource_id: "res:svc-voice-fixture".into(),
            connection_id: "conn:svc-voice-fixture".into(),
            capability: Capability::Transcription,
            model: "pinned-asr".into(),
            detail: Some("ja".into()),
            request_options: None,
            enabled: true,
        });
        let binding = snapshot
            .bindings
            .iter_mut()
            .find(|b| b.purpose == Purpose::VoiceTranscribe)
            .unwrap();
        binding.primary_resource_id = Some("res:svc-voice-fixture".into());
        binding.review = BindingReview::Ready;
        persistence::service_registry_store::save_registry(&mut db, &snapshot, loaded.revision)
            .unwrap();
        db
    }
    #[tokio::test]
    async fn custom_asr_keeps_the_same_provider_across_partial_and_final_after_selection_changes() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let observed = calls.clone();
        let router = axum::Router::new().route(
            "/v1/audio/transcriptions",
            axum::routing::post(move |bytes: axum::body::Bytes| {
                let calls = observed.clone();
                async move {
                    assert!(bytes
                        .windows("pinned-asr".len())
                        .any(|part| part == b"pinned-asr"));
                    calls.fetch_add(1, Ordering::SeqCst);
                    axum::Json(json!({"text":"人の発話","language":"ja"}))
                }
            }),
        );
        let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        let db = database(&format!("http://{address}/v1"));
        let state = crate::test_support::app_state(db);
        let id = format!("voice_{}", uuid::Uuid::new_v4().simple());
        let audit = ConversationAudit::new(state.sqlite_writer.clone(), id.clone());
        let upload = state.audio_uploads.stage_pcm_for_e2e(&[1000_i16; 3200]);
        let first = super::super::transcribe_conversation_audio_inner(
            &state,
            super::super::TranscribeInput {
                audio_upload_id: upload,
                utterance_id: id.clone(),
                kind: "partial".into(),
            },
            &audit,
        )
        .await
        .unwrap();
        assert_eq!(first.text, "人の発話");
        state
            .sqlite_writer
            .write(|db| {
                let loaded = persistence::service_registry_store::load_registry(db)?;
                let mut next = loaded.snapshot;
                next.bindings
                    .iter_mut()
                    .find(|b| b.purpose == Purpose::VoiceTranscribe)
                    .unwrap()
                    .primary_resource_id = Some("res:harness-asr".into());
                persistence::service_registry_store::save_registry(db, &next, loaded.revision)?;
                Ok(())
            })
            .unwrap();
        let upload = state.audio_uploads.stage_pcm_for_e2e(&[1000_i16; 3200]);
        let final_result = super::super::transcribe_conversation_audio_inner(
            &state,
            super::super::TranscribeInput {
                audio_upload_id: upload,
                utterance_id: id.clone(),
                kind: "final".into(),
            },
            &audit,
        )
        .await
        .unwrap();
        assert_eq!(final_result.text, "人の発話");
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        assert!(utterance(&state, &id).is_err());
        // Eviction/restart cannot re-pin a completed ID to changed settings.
        UTTERANCES
            .get()
            .unwrap()
            .lock()
            .unwrap()
            .retain(|r| r.id != id);
        assert!(utterance(&state, &id).is_err());
        let next_id = format!("voice_{}", uuid::Uuid::new_v4().simple());
        assert_eq!(
            utterance(&state, &next_id).unwrap().resolved.resource_id,
            "res:harness-asr"
        );
        server.abort();
    }
    #[tokio::test]
    async fn withdrawing_permission_or_disabling_the_connection_rejects_a_pinned_voice_route() {
        let state = crate::test_support::app_state(database("http://127.0.0.1:1/v1"));
        let prepared = state
            .sqlite_readers
            .read(|db| prepare(db, Purpose::VoiceTranscribe))
            .unwrap();
        state
            .sqlite_writer
            .write(|db| {
                let loaded = persistence::service_registry_store::load_registry(db)?;
                let mut next = loaded.snapshot;
                next.bindings
                    .iter_mut()
                    .find(|b| b.purpose == Purpose::VoiceTranscribe)
                    .unwrap()
                    .cloud_allowed = false;
                persistence::service_registry_store::save_registry(db, &next, loaded.revision)?;
                Ok(())
            })
            .unwrap();
        assert!(prepared.validate(&state).is_err());
        assert!(state
            .sqlite_readers
            .read(|db| prepare(db, Purpose::VoiceTranscribe))
            .is_err());
    }
}
