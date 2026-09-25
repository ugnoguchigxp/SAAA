use super::item;
use crate::diagnosis::contract::{DiagnosisItem, DiagnosisSeverity, DiagnosisStatus};
use crate::persistence::{load_model_providers, load_routing_settings};
use crate::providers::service_harness::{HarnessResolution, HarnessServiceStatus};
use crate::voice::network_asr::NetworkAsrRuntime;
use crate::AppState;
use rusqlite::OptionalExtension;
use std::{sync::Arc, time::Duration};

pub(in crate::diagnosis) async fn harness(state: &AppState) -> Vec<DiagnosisItem> {
    let settings = match state.sqlite_readers.read(load_model_providers) {
        Ok(settings) => settings,
        Err(_) => return Vec::new(),
    };
    if settings.harness.address.trim().is_empty() {
        return vec![
            item(
                "harness.reachability",
                "harness",
                "Harness reachability",
                DiagnosisStatus::Skipped,
                DiagnosisSeverity::Degraded,
                "harness address is empty",
                None,
            ),
            item(
                "harness.resolve",
                "harness",
                "Harness resolve",
                DiagnosisStatus::Skipped,
                DiagnosisSeverity::Degraded,
                "harness address is empty",
                None,
            ),
        ];
    }
    let legacy =
        crate::providers::service_harness::legacy_dynamic_lan_host(&settings.harness.address)
            .ok()
            .flatten()
            .is_some();
    let uses_harness_asr = state
        .sqlite_readers
        .read(load_routing_settings)
        .is_ok_and(|routing| routing.voice_transcribe.source == "harness");
    let recent_asr = recent_asr_result(state);
    let legacy_asr = async {
        if uses_harness_asr {
            legacy_asr_item(&settings.harness.address, recent_asr).await
        } else {
            None
        }
    };
    let stored_profile = settings.harness.larm_profile.as_deref();
    let preference = crate::larm_voice::profile::preference(stored_profile);
    let advertised = async {
        if legacy {
            advertised_llms(&settings.harness.address, &preference).await
        } else {
            None
        }
    };
    let resolution = crate::providers::service_harness::resolve_with_legacy_llm(
        &settings.harness.address,
        stored_profile,
    );
    let (resolution, legacy_asr, advertised) = tokio::join!(resolution, legacy_asr, advertised);
    let mut items = Vec::new();
    match resolution {
        Ok(resolution) => {
            let readiness = readiness_item(
                legacy,
                true,
                "Agent connection and model readiness probe succeeded",
            );
            items.push(readiness.clone());
            items.extend(items_from_resolution(&resolution));
            if let Some(detail) = advertised.as_deref() {
                if let Some(llm) = items.iter_mut().find(|item| item.id == "harness.llm") {
                    llm.message = detail.to_string();
                }
            }
            if readiness.status == DiagnosisStatus::Ok {
                promote_response(&mut items, &readiness.message);
            }
            if legacy {
                let (embedding, binding) =
                    probe_embedding(&settings.harness.address, preference).await;
                items.retain(|item| item.id != "harness.embedding");
                items.push(embedding);
                items.extend(binding);
            }
        }
        Err(error) => {
            let message = match advertised.as_deref() {
                Some(detail) => format!("{detail}\n{error}"),
                None => error,
            };
            push_failure(&mut items, legacy, &message);
        }
    }
    if let Some(legacy_asr) = legacy_asr {
        items.retain(|item| item.id != "harness.asr");
        items.push(legacy_asr);
    }
    items
}

fn advertised_detail(catalog: &saaa_larm_session::catalog::CatalogProfile) -> Option<String> {
    let lines = ["llm", "backchannel"]
        .into_iter()
        .filter_map(|name| {
            catalog
                .provider(name)
                .map(|provider| format!("{name}: {} · {}", provider.model, provider.endpoint))
        })
        .collect::<Vec<_>>();
    if lines.is_empty() {
        None
    } else {
        Some(lines.join("\n"))
    }
}

async fn advertised_llms(
    address: &str,
    preference: &saaa_larm_session::ProfilePreference,
) -> Option<String> {
    let saaa_larm_session::ProfilePreference::Variant(variant) = preference else {
        return None;
    };
    let host = crate::providers::service_harness::legacy_dynamic_lan_host(address)
        .ok()
        .flatten()?;
    let base = crate::providers::dynamic_lan::control_base_url(&host).ok()?;
    let credential = crate::providers::dynamic_lan::credential::load().ok()?;
    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(8))
        .redirect(reqwest::redirect::Policy::none())
        .no_proxy()
        .build()
        .ok()?;
    let catalog = tokio::time::timeout(
        Duration::from_secs(8),
        saaa_larm_session::catalog::fetch(&client, &base, credential.token(), variant.selector()),
    )
    .await
    .ok()?
    .ok()?;
    advertised_detail(&catalog)
}

fn push_failure(items: &mut Vec<DiagnosisItem>, legacy: bool, message: &str) {
    if legacy {
        items.push(readiness_item(true, false, message));
        return;
    }
    items.push(readiness_item(
        false,
        false,
        "not an Agent Connection address",
    ));
    items.push(item(
        "harness.resolve",
        "harness",
        "Harness resolve",
        DiagnosisStatus::Fail,
        DiagnosisSeverity::Degraded,
        message,
        None,
    ));
}

fn readiness_item(legacy: bool, ready: bool, message: &str) -> DiagnosisItem {
    if !legacy {
        return item(
            "harness.reachability",
            "harness",
            "Harness reachability",
            DiagnosisStatus::Skipped,
            DiagnosisSeverity::Degraded,
            message,
            None,
        );
    }
    item(
        "harness.reachability",
        "harness",
        "Harness reachability",
        if ready {
            DiagnosisStatus::Ok
        } else {
            DiagnosisStatus::Fail
        },
        DiagnosisSeverity::Degraded,
        message,
        None,
    )
}

fn recent_asr_result(state: &AppState) -> Option<(String, String)> {
    let cutoff = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_millis()
        .saturating_sub(5 * 60 * 1_000) as i64;
    state
        .sqlite_readers
        .read(|connection| {
            connection
                .query_row(
                    "SELECT event_name, COALESCE(failure_code, '') FROM audit_events \
                     WHERE component='voice-asr' AND CAST(occurred_at AS INTEGER)>=?1 \
                     AND event_name IN ('capture-start-failed', 'asr-utterance-discarded', \
                     'asr-final-received', 'asr-ready') ORDER BY sequence DESC LIMIT 1",
                    [cutoff],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()
                .map_err(|error| error.to_string())
        })
        .ok()
        .flatten()
}

fn observed_asr_status(event: &(String, String)) -> (DiagnosisStatus, &'static str) {
    match (event.0.as_str(), event.1.as_str()) {
        ("asr-final-received", _) => (DiagnosisStatus::Ok, "Recent speech was transcribed"),
        ("capture-start-failed", _) => (DiagnosisStatus::Fail, "Recent microphone capture failed"),
        ("asr-utterance-discarded", "target-speaker-empty") => (
            DiagnosisStatus::Fail,
            "Recent speech was discarded by the target-speaker filter",
        ),
        ("asr-utterance-discarded", _) => (DiagnosisStatus::Warn, "Recent speech was discarded"),
        _ => (
            DiagnosisStatus::Warn,
            "ASR started, but no transcription was confirmed",
        ),
    }
}

async fn legacy_asr_item(address: &str, recent: Option<(String, String)>) -> Option<DiagnosisItem> {
    let host = crate::providers::service_harness::legacy_dynamic_lan_host(address)
        .ok()
        .flatten()?;
    let probe = async {
        let cancellation = Arc::new(crate::RunCancellation::default());
        if crate::providers::service_harness::resolve_service_cancellable(
            address,
            "asr",
            &cancellation,
        )
        .await
        .is_ok()
        {
            return Ok(());
        }
        NetworkAsrRuntime::new()?
            .resolve(&host, cancellation)
            .await
            .map(|_| ())
    };
    let (status, message) = match tokio::time::timeout(Duration::from_secs(8), probe).await {
        Ok(Ok(())) => recent.as_ref().map(observed_asr_status).unwrap_or((
            DiagnosisStatus::Warn,
            "ASR endpoint is reachable; microphone capture and transcription were not tested",
        )),
        Ok(Err(_)) => (DiagnosisStatus::Fail, "ASR service is unavailable"),
        Err(_) => (
            DiagnosisStatus::Fail,
            "ASR service did not respond within 8s",
        ),
    };
    Some(item(
        "harness.asr",
        "voice",
        "Harness ASR",
        status,
        DiagnosisSeverity::Degraded,
        message,
        None,
    ))
}

pub(super) fn items_from_resolution(resolution: &HarnessResolution) -> Vec<DiagnosisItem> {
    let mut items = resolution
        .services
        .iter()
        .map(service_item)
        .collect::<Vec<_>>();
    for capability in ["llm", "asr", "tts", "embedding"] {
        if items
            .iter()
            .any(|item| item.id == format!("harness.{capability}"))
        {
            continue;
        }
        items.push(item(
            &format!("harness.{capability}"),
            "harness",
            &format!("Harness {capability}"),
            DiagnosisStatus::Skipped,
            DiagnosisSeverity::Info,
            &format!("{capability} is not advertised"),
            None,
        ));
    }
    items
}

async fn probe_embedding(
    address: &str,
    preference: saaa_larm_session::ProfilePreference,
) -> (DiagnosisItem, Vec<DiagnosisItem>) {
    let probed = tokio::time::timeout(
        Duration::from_secs(8),
        request_embedding(address, preference),
    )
    .await;
    let (status, message, binding) = match probed {
        Ok(Ok(binding)) => (
            DiagnosisStatus::Ok,
            "Embedding request returned a vector".to_string(),
            binding,
        ),
        Ok(Err(error)) => (DiagnosisStatus::Fail, error, Vec::new()),
        Err(_) => (
            DiagnosisStatus::Fail,
            "Embedding did not respond within 8s".to_string(),
            Vec::new(),
        ),
    };
    (
        item(
            "harness.embedding",
            "harness",
            "Harness embedding",
            status,
            DiagnosisSeverity::Degraded,
            &message,
            None,
        ),
        binding,
    )
}

fn binding_items(session: &saaa_larm_session::Session) -> Vec<DiagnosisItem> {
    let selector = session.selector().unwrap_or("explicit");
    let revision = session.catalog_revision().unwrap_or("none");
    vec![
        item(
            "harness.larm.selector",
            "harness",
            "LARM selector",
            DiagnosisStatus::Ok,
            DiagnosisSeverity::Info,
            selector,
            None,
        ),
        item(
            "harness.larm.catalog-revision",
            "harness",
            "LARM catalog revision",
            DiagnosisStatus::Ok,
            DiagnosisSeverity::Info,
            revision,
            None,
        ),
    ]
}

async fn request_embedding(
    address: &str,
    preference: saaa_larm_session::ProfilePreference,
) -> Result<Vec<DiagnosisItem>, String> {
    let credential = crate::providers::dynamic_lan::credential::load()
        .map_err(|error| error.code().to_string())?;
    let (_stop, cancel) = tokio::sync::watch::channel(false);
    let session = saaa_larm_session::Session::connect_with_profile_credential_and_key(
        address,
        preference,
        credential.token().to_string(),
        format!("saaa-session-{}", uuid::Uuid::new_v4()),
        cancel,
    )
    .await
    .map_err(|error| error.to_string())?;
    let mut binding = binding_items(&session);
    let summary = session.provider_summary().await;
    for name in ["llm", "backchannel"] {
        let Some(provider) = summary.iter().find(|provider| provider.name == name) else {
            continue;
        };
        let max_tokens = provider
            .context_window
            .map(|window| window.max_tokens.to_string())
            .unwrap_or_else(|| "none".to_string());
        binding.push(item(
            &format!("harness.larm.{name}"),
            "harness",
            &format!("LARM {name}"),
            DiagnosisStatus::Ok,
            DiagnosisSeverity::Info,
            &format!(
                "model={} endpoint={} maxTokens={max_tokens}",
                provider.model, provider.endpoint
            ),
            None,
        ));
    }
    let embedded = session.embed_query(&["診断".to_string()]).await;
    let _ = session.close().await;
    let vectors = embedded?;
    if vectors.first().is_some_and(|vector| !vector.is_empty()) {
        Ok(binding)
    } else {
        Err("Embedding response was empty".to_string())
    }
}

fn promote_response(items: &mut Vec<DiagnosisItem>, message: &str) {
    if let Some(llm) = items.iter_mut().find(|item| item.id == "harness.llm") {
        if llm.status == DiagnosisStatus::Skipped {
            llm.status = DiagnosisStatus::Ok;
            llm.severity = DiagnosisSeverity::Degraded;
            llm.message = message.to_string();
        }
        return;
    }
    items.push(item(
        "harness.llm",
        "harness",
        "Harness llm",
        DiagnosisStatus::Ok,
        DiagnosisSeverity::Degraded,
        message,
        None,
    ));
}

fn service_item(service: &HarnessServiceStatus) -> DiagnosisItem {
    let embedding = service.capability == "embedding";
    let unadvertised = service.state == "unavailable"
        && service
            .message
            .to_ascii_lowercase()
            .contains("not advertised");
    let status = if unadvertised {
        DiagnosisStatus::Skipped
    } else {
        match service.state {
            "ready" => DiagnosisStatus::Ok,
            "degraded" => DiagnosisStatus::Warn,
            "unavailable" => DiagnosisStatus::Fail,
            _ => DiagnosisStatus::Warn,
        }
    };
    item(
        &format!("harness.{}", service.capability),
        "harness",
        &format!("Harness {}", service.capability),
        status,
        if embedding {
            DiagnosisSeverity::Info
        } else {
            DiagnosisSeverity::Degraded
        },
        &service.message,
        None,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    fn fresh() -> AppState {
        let connection = Connection::open_in_memory().expect("database opens");
        crate::initialize_database(&connection).expect("database initializes");
        crate::test_support::app_state(connection)
    }

    fn write_address(state: &AppState, address: &str) {
        state
            .sqlite_writer
            .write(|connection| {
                connection
                    .execute(
                        "UPDATE settings_documents
                         SET value_json=json_set(value_json, '$.harness.address', ?1)
                         WHERE namespace='providers.model' AND key='default'",
                        [address],
                    )
                    .map_err(crate::database_error)?;
                Ok(())
            })
            .expect("address update");
    }

    #[tokio::test]
    async fn dg_06_harness_skipped_when_address_empty() {
        let state = fresh();
        write_address(&state, "");
        let items = harness(&state).await;
        assert!(items.len() >= 2);
        assert!(items
            .iter()
            .all(|item| item.status == DiagnosisStatus::Skipped));
        assert!(items.iter().any(|item| item.id == "harness.reachability"));
        assert!(items.iter().any(|item| item.id == "harness.resolve"));
    }

    #[tokio::test]
    async fn dg_06_harness_resolve_error_yields_single_fail() {
        let state = fresh();
        write_address(&state, "http://127.0.0.1:9/");
        let items = harness(&state).await;
        let failures = items
            .iter()
            .filter(|item| item.status == DiagnosisStatus::Fail)
            .collect::<Vec<_>>();
        assert_eq!(failures.len(), 1);
        assert_eq!(failures[0].id, "harness.resolve");
        assert!(items.iter().all(|item| !item.id.starts_with("harness.llm")));
    }

    #[test]
    fn dg_06_embedding_maps_ready_and_missing() {
        let ready = items_from_resolution(&HarnessResolution {
            state: "ready",
            revision: "rev".into(),
            services: vec![HarnessServiceStatus {
                capability: "embedding",
                state: "ready",
                protocol: None,
                model: Some("embed".into()),
                language: None,
                voice: None,
                message: "ready".into(),
            }],
        });
        assert_eq!(ready.len(), 4);
        let embedding = ready
            .iter()
            .find(|item| item.id == "harness.embedding")
            .expect("embedding");
        assert_eq!(embedding.status, DiagnosisStatus::Ok);
        assert_eq!(embedding.severity, DiagnosisSeverity::Info);
        assert!(ready.iter().any(|item| item.id == "harness.asr"));

        let mut missing = items_from_resolution(&HarnessResolution {
            state: "degraded",
            revision: "rev".into(),
            services: vec![],
        });
        assert_eq!(missing.len(), 4);
        assert!(missing
            .iter()
            .all(|item| item.status == DiagnosisStatus::Skipped));
        promote_response(
            &mut missing,
            "Agent connection and model readiness probe succeeded",
        );
        assert_eq!(
            missing
                .iter()
                .find(|item| item.id == "harness.llm")
                .map(|item| item.status),
            Some(DiagnosisStatus::Ok)
        );
    }

    #[test]
    fn recent_target_speaker_discard_is_an_asr_failure() {
        assert_eq!(
            observed_asr_status(&(
                "asr-utterance-discarded".into(),
                "target-speaker-empty".into(),
            ))
            .0,
            DiagnosisStatus::Fail
        );
        assert_eq!(
            observed_asr_status(&(String::from("asr-ready"), String::new())).0,
            DiagnosisStatus::Warn
        );
    }

    #[test]
    fn advertised_detail_shows_model_and_endpoint() {
        let catalog = saaa_larm_session::catalog::CatalogProfile {
            revision: "rev".into(),
            selector: "SAAA".into(),
            id: "saaa-conversation-ornith15".into(),
            providers: vec![
                saaa_larm_session::catalog::CatalogProvider {
                    name: "llm".into(),
                    capability: "llm.general".into(),
                    protocol: "openai.chat-completions.v1".into(),
                    endpoint: "/v1/chat/completions".into(),
                    model: "ornith-1.5-35b".into(),
                    context_window: None,
                },
                saaa_larm_session::catalog::CatalogProvider {
                    name: "backchannel".into(),
                    capability: "llm.backchannel.classifier".into(),
                    protocol: "openai.chat-completions.v1".into(),
                    endpoint: "/v1/chat/completions".into(),
                    model: "qwen3.5-2b-fast-response".into(),
                    context_window: None,
                },
            ],
            services: Vec::new(),
        };
        assert_eq!(
            advertised_detail(&catalog).as_deref(),
            Some(
                "llm: ornith-1.5-35b · /v1/chat/completions\nbackchannel: qwen3.5-2b-fast-response · /v1/chat/completions"
            )
        );
    }

    #[test]
    fn unadvertised_speech_is_not_a_failure() {
        let items = items_from_resolution(&HarnessResolution {
            state: "degraded",
            revision: "agent-connection.v1".into(),
            services: vec![HarnessServiceStatus {
                capability: "tts",
                state: "unavailable",
                protocol: None,
                model: None,
                language: None,
                voice: None,
                message: "Capability is not advertised by this Harness".into(),
            }],
        });
        assert_eq!(
            items
                .iter()
                .find(|item| item.id == "harness.tts")
                .map(|item| item.status),
            Some(DiagnosisStatus::Skipped)
        );
    }
}
