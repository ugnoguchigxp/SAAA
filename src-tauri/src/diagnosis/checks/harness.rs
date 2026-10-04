use super::item;
use crate::diagnosis::contract::{DiagnosisItem, DiagnosisSeverity, DiagnosisStatus};
use crate::persistence::{load_model_providers, load_routing_settings};
use crate::providers::service_harness::{HarnessResolution, HarnessServiceStatus};
use crate::AppState;
use rusqlite::OptionalExtension;
use std::time::Duration;

/// Reads the control-plane catalog without allocating a Connection or waking models.
pub(in crate::diagnosis) async fn fast(state: &AppState) -> Vec<DiagnosisItem> {
    let settings = match state.sqlite_readers.read(load_model_providers) {
        Ok(settings) => settings,
        Err(_) => return Vec::new(),
    };
    if settings.harness.address.trim().is_empty() {
        return vec![item(
            "harness.reachability",
            "harness",
            "LARM reachability",
            DiagnosisStatus::Skipped,
            DiagnosisSeverity::Info,
            "Agent Connection address is not configured",
            None,
        )];
    }
    let started = std::time::Instant::now();
    let result =
        match crate::providers::service_harness::legacy_dynamic_lan_host(&settings.harness.address)
        {
            Ok(Some(host)) => {
                let result = async {
                    let base = crate::providers::dynamic_lan::control_base_url(&host)
                        .map_err(|error| error.public_message().to_string())?;
                    let credential = crate::providers::dynamic_lan::credential::load()
                        .map_err(|error| error.code().to_string())?;
                    let client = reqwest::Client::builder()
                        .connect_timeout(Duration::from_secs(3))
                        .timeout(Duration::from_secs(5))
                        .no_proxy()
                        .build()
                        .map_err(|_| "Could not initialize the LARM client".to_string())?;
                    let preference = crate::providers::larm_resources::profile::preference(
                        settings.harness.larm_profile.as_deref(),
                    );
                    let selector = match preference {
                        saaa_larm_session::ProfilePreference::Variant(variant) => {
                            variant.selector().to_string()
                        }
                        saaa_larm_session::ProfilePreference::Explicit(value) => value,
                    };
                    saaa_larm_session::catalog::fetch(&client, &base, credential.token(), &selector)
                        .await
                        .map_err(str::to_string)
                }
                .await;
                result.map(|catalog| {
                    let mut items = vec![item(
                        "harness.reachability",
                        "harness",
                        "LARM catalog",
                        DiagnosisStatus::Ok,
                        DiagnosisSeverity::Degraded,
                        "LARM catalog responded; execution was not tested",
                        Some(started.elapsed().as_millis() as u64),
                    )];
                    for capability in ["llm", "backchannel", "asr", "tts", "embedding"] {
                        let advertised = catalog
                            .providers
                            .iter()
                            .any(|provider| provider.name == capability);
                        items.push(item(
                            &format!("harness.{capability}"),
                            "harness",
                            &format!("LARM {capability}"),
                            if advertised {
                                DiagnosisStatus::Skipped
                            } else {
                                DiagnosisStatus::Warn
                            },
                            DiagnosisSeverity::Info,
                            if advertised {
                                "Advertised; run operational diagnosis to test readiness"
                            } else {
                                "Not advertised in the selected profile"
                            },
                            None,
                        ));
                    }
                    items
                })
            }
            Ok(None) => crate::providers::service_harness::resolve(&settings.harness.address)
                .await
                .map(|resolution| items_from_resolution(&resolution)),
            Err(error) => Err(error),
        };
    result.unwrap_or_else(|error| {
        vec![item(
            "harness.reachability",
            "harness",
            "LARM catalog",
            DiagnosisStatus::Fail,
            DiagnosisSeverity::Degraded,
            &error,
            Some(started.elapsed().as_millis() as u64),
        )]
    })
}

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
    let resolution_started = std::time::Instant::now();
    let resolution = crate::providers::service_harness::resolve_for_diagnosis(
        &settings.harness.address,
        stored_profile,
    );
    let resolution = resolution.await;
    let mut items = Vec::new();
    match resolution {
        Ok(resolution) => {
            let readiness = readiness_item(legacy, true, "Agent Connection and claim succeeded");
            items.push(readiness.clone());
            items.extend(items_from_resolution(&resolution));
            if let Some(timings) = resolution.diagnosis_timings.as_ref() {
                for (capability, elapsed) in &timings.provider_ms {
                    if let Some(service) = items
                        .iter_mut()
                        .find(|item| item.id == format!("harness.{capability}"))
                    {
                        service.latency_ms = Some(*elapsed);
                    }
                }
                for (stage, label, elapsed) in [
                    ("connection", "Connection and claim", timings.connect_ms),
                    (
                        "embedding-request",
                        "Embedding request",
                        timings.embedding_ms,
                    ),
                    ("release", "Connection release", timings.release_ms),
                ] {
                    items.push(item(
                        &format!("harness.larm.{stage}"),
                        "harness",
                        label,
                        DiagnosisStatus::Ok,
                        DiagnosisSeverity::Info,
                        "Completed",
                        Some(elapsed),
                    ));
                }
            }
            if let Some(event) = recent_asr.as_ref() {
                let (status, message) = observed_asr_status(event);
                items.push(item(
                    "harness.recent-asr",
                    "harness",
                    "Recent microphone observation",
                    status,
                    DiagnosisSeverity::Info,
                    message,
                    None,
                ));
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
            if let Some(resolve) = items.iter_mut().find(|item| item.id == "harness.resolve") {
                resolve.latency_ms = Some(resolution_started.elapsed().as_millis() as u64);
            }
        }
    }
    items
}

#[path = "harness/items.rs"]
mod items;
use items::*;
