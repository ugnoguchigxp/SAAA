use super::item;
use crate::diagnosis::contract::{DiagnosisItem, DiagnosisSeverity, DiagnosisStatus};
use crate::persistence::{load_model_providers, load_routing_settings};
use crate::providers::service_harness::{HarnessResolution, HarnessServiceStatus};
use crate::AppState;
use rusqlite::OptionalExtension;
use std::time::Duration;

pub(in crate::diagnosis) async fn harness(state: &AppState) -> Vec<DiagnosisItem> {
    let settings = match state.sqlite_readers.read(load_model_providers) {
        Ok(settings) => settings,
        Err(_) => return Vec::new(),
    };
    if settings.harness.address.trim().is_empty() {
        let mut items = vec![
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
        for capability in ["llm", "backchannel", "asr", "tts", "embedding"] {
            items.push(item(
                &format!("harness.{capability}"),
                "harness",
                &format!("LARM {capability}"),
                DiagnosisStatus::Skipped,
                DiagnosisSeverity::Info,
                "Agent Connection address is not configured",
                None,
            ));
        }
        return items;
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
    let recent_asr = uses_harness_asr.then(|| recent_asr_result(state)).flatten();
    let stored_profile = settings.harness.larm_profile.as_deref();
    let resolution = crate::providers::service_harness::resolve_with_legacy_llm(
        &settings.harness.address,
        stored_profile,
    );
    let resolution = resolution.await;
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
            if legacy {
                if let Some(asr) = items.iter_mut().find(|item| item.id == "harness.asr") {
                    if let Some(event) = recent_asr.as_ref() {
                        let (status, message) = observed_asr_status(event);
                        asr.status = status;
                        asr.message = message.into();
                    } else {
                        asr.message = "ASR Provider ready; microphone capture and transcription were not tested".into();
                    }
                }
            }
            if legacy {
                items.push(item(
                    "harness.tcp",
                    "harness",
                    "LARM TCP reachability",
                    DiagnosisStatus::Ok,
                    DiagnosisSeverity::Info,
                    "LARM TCP connection succeeded",
                    None,
                ));
                for (stage, label) in [
                    ("discovery", "LARM discovery"),
                    ("create", "Agent Connection create"),
                    ("semantic-readiness", "Provider semantic readiness"),
                    ("claim", "Provider claim"),
                ] {
                    items.push(item(
                        &format!("harness.larm.{stage}"),
                        "harness",
                        label,
                        DiagnosisStatus::Ok,
                        DiagnosisSeverity::Info,
                        "Verified by the Agent Connection resolution",
                        None,
                    ));
                }
            }
        }
        Err(error) => {
            let message = error;
            let host = if legacy {
                crate::providers::service_harness::legacy_dynamic_lan_host(
                    &settings.harness.address,
                )
                .ok()
                .flatten()
            } else {
                None
            };
            let reached_tcp = if let Some(host) = host.as_deref() {
                tokio::time::timeout(
                    Duration::from_secs(3),
                    tokio::net::TcpStream::connect((
                        host,
                        crate::providers::dynamic_lan::CONTROL_PORT,
                    )),
                )
                .await
                .is_ok_and(|result| result.is_ok())
            } else {
                false
            };
            let reached_http = if let Some(host) = host.as_deref() {
                crate::providers::dynamic_lan::probe::reachable(host, Duration::from_secs(3)).await
            } else {
                false
            };
            push_failure(&mut items, legacy, reached_tcp, reached_http, &message);
        }
    }
    items
}

fn push_failure(
    items: &mut Vec<DiagnosisItem>,
    legacy: bool,
    reached_tcp: bool,
    reached_http: bool,
    message: &str,
) {
    if legacy {
        items.push(item(
            "harness.tcp",
            "harness",
            "LARM TCP reachability",
            if reached_tcp {
                DiagnosisStatus::Ok
            } else {
                DiagnosisStatus::Fail
            },
            DiagnosisSeverity::Degraded,
            if reached_tcp {
                "LARM TCP connection succeeded"
            } else {
                "LARM TCP connection failed"
            },
            None,
        ));
        items.push(item(
            "harness.reachability",
            "harness",
            "Harness reachability",
            if reached_http {
                DiagnosisStatus::Ok
            } else {
                DiagnosisStatus::Fail
            },
            DiagnosisSeverity::Degraded,
            if reached_http {
                "LARM HTTP endpoint responded"
            } else {
                "LARM HTTP endpoint did not respond"
            },
            None,
        ));
        items.push(item(
            "harness.resolve",
            "harness",
            "Agent Connection create and readiness",
            DiagnosisStatus::Fail,
            DiagnosisSeverity::Degraded,
            message,
            None,
        ));
        for capability in ["llm", "backchannel", "asr", "tts", "embedding"] {
            items.push(item(
                &format!("harness.{capability}"),
                "harness",
                &format!("LARM {capability}"),
                DiagnosisStatus::Fail,
                DiagnosisSeverity::Degraded,
                message,
                None,
            ));
        }
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
            DiagnosisStatus::Ok,
            "ASR Provider is ready; transcription was not yet confirmed",
        ),
    }
}

pub(super) fn items_from_resolution(resolution: &HarnessResolution) -> Vec<DiagnosisItem> {
    let mut items = resolution
        .services
        .iter()
        .map(service_item)
        .collect::<Vec<_>>();
    for capability in ["llm", "backchannel", "asr", "tts", "embedding"] {
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

    #[test]
    fn http_success_and_create_contract_failure_are_separate_diagnostics() {
        let mut items = Vec::new();
        push_failure(&mut items, true, true, true, "larm_invalid_request");
        assert_eq!(
            items
                .iter()
                .find(|item| item.id == "harness.tcp")
                .map(|item| item.status),
            Some(DiagnosisStatus::Ok)
        );
        assert_eq!(
            items
                .iter()
                .find(|item| item.id == "harness.reachability")
                .map(|item| item.status),
            Some(DiagnosisStatus::Ok)
        );
        assert_eq!(
            items
                .iter()
                .find(|item| item.id == "harness.resolve")
                .map(|item| item.status),
            Some(DiagnosisStatus::Fail)
        );
        for capability in ["llm", "backchannel", "asr", "tts", "embedding"] {
            assert_eq!(
                items
                    .iter()
                    .find(|item| item.id == format!("harness.{capability}"))
                    .map(|item| item.status),
                Some(DiagnosisStatus::Fail)
            );
        }
    }
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
        assert_eq!(ready.len(), 5);
        let embedding = ready
            .iter()
            .find(|item| item.id == "harness.embedding")
            .expect("embedding");
        assert_eq!(embedding.status, DiagnosisStatus::Ok);
        assert_eq!(embedding.severity, DiagnosisSeverity::Info);
        assert!(ready.iter().any(|item| item.id == "harness.asr"));

        let missing = items_from_resolution(&HarnessResolution {
            state: "degraded",
            revision: "rev".into(),
            services: vec![],
        });
        assert_eq!(missing.len(), 5);
        assert!(missing
            .iter()
            .all(|item| item.status == DiagnosisStatus::Skipped));
        assert_eq!(
            missing
                .iter()
                .find(|item| item.id == "harness.llm")
                .map(|item| item.status),
            Some(DiagnosisStatus::Skipped)
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
            DiagnosisStatus::Ok
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
