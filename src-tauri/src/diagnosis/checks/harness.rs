//! The LARM link. Every LARM service is one route of the capability it serves:
//! llm and backchannel -> conversation, asr -> voice-listen, tts -> voice-speak,
//! embedding -> memory (advisory).
//!
//! `catalog` only reads the control plane, so "advertised" is never proof. `session`
//! claims one Session and exercises each service.
use super::{classify_failure, evidence, CheckFuture};
use crate::diagnosis::contract::{Capability, Evidence, Importance, Outcome, Reason, Tier};
use crate::persistence::load_model_providers;
use crate::providers::service_harness::{HarnessResolution, HarnessServiceStatus};
use crate::AppState;
use std::time::Duration;

const SERVICES: [&str; 5] = ["llm", "backchannel", "asr", "tts", "embedding"];
const ROUTE: &str = "larm";

fn target(service: &str) -> Option<(Capability, Importance)> {
    match service {
        "llm" => Some((Capability::Conversation, Importance::Required)),
        "backchannel" => Some((Capability::Conversation, Importance::Advisory)),
        "asr" => Some((Capability::VoiceListen, Importance::Required)),
        "tts" => Some((Capability::VoiceSpeak, Importance::Required)),
        "embedding" => Some((Capability::Memory, Importance::Advisory)),
        _ => None,
    }
}

fn service_evidence(
    service: &str,
    tier: Tier,
    outcome: Outcome,
    reason: Reason,
) -> Option<Evidence> {
    let (capability, importance) = target(service)?;
    let mut item = evidence(
        &format!("larm.{service}"),
        capability,
        tier,
        outcome,
        reason,
    )
    .route(ROUTE);
    item.importance = importance;
    Some(item)
}

fn all_services(tier: Tier, outcome: Outcome, reason: Reason, detail: &str) -> Vec<Evidence> {
    SERVICES
        .iter()
        .filter_map(|service| service_evidence(service, tier, outcome, reason))
        .map(|item| item.detail(detail))
        .collect()
}

/// A missing optional service (backchannel, embedding) is not a fault: the link simply does
/// not offer it. A missing required service is.
fn absent(service: &str, tier: Tier) -> Option<Evidence> {
    match target(service)? {
        (_, Importance::Advisory) => {
            service_evidence(service, tier, Outcome::Disabled, Reason::NotAdvertised)
        }
        (_, Importance::Required) => {
            service_evidence(service, tier, Outcome::Fail, Reason::NotAdvertised)
        }
    }
}

fn from_advertised(advertised: &[&str]) -> Vec<Evidence> {
    SERVICES
        .iter()
        .filter_map(|service| {
            if advertised.contains(service) {
                service_evidence(service, Tier::Static, Outcome::Pass, Reason::Ok)
            } else {
                absent(service, Tier::Static)
            }
        })
        .collect()
}

fn outcome_of(service: &HarnessServiceStatus) -> (Outcome, Reason) {
    let unadvertised = service.state == "unavailable"
        && service
            .message
            .to_ascii_lowercase()
            .contains("not advertised");
    if unadvertised {
        return (Outcome::Fail, Reason::NotAdvertised);
    }
    match service.state {
        "ready" => (Outcome::Pass, Reason::Ok),
        "unavailable" => (Outcome::Fail, classify_failure(&service.message)),
        _ => (Outcome::Degraded, Reason::NotReady),
    }
}

fn from_resolution(
    resolution: &HarnessResolution,
    tier: Tier,
    latency: &[(&'static str, u64)],
) -> Vec<Evidence> {
    SERVICES
        .iter()
        .filter_map(|name| {
            let status = resolution
                .services
                .iter()
                .find(|service| service.capability == *name);
            let Some(service) = status else {
                return absent(name, tier);
            };
            let (outcome, reason) = outcome_of(service);
            let ms = latency.iter().find(|(c, _)| c == name).map(|(_, ms)| *ms);
            let item = service_evidence(name, tier, outcome, reason)?.latency(ms);
            Some(if outcome == Outcome::Pass {
                item
            } else {
                item.detail(&service.message)
            })
        })
        .collect()
}

pub(super) fn catalog(state: &AppState) -> CheckFuture<'_> {
    Box::pin(async move {
        let Ok(settings) = state.sqlite_readers.read(load_model_providers) else {
            return Vec::new();
        };
        let address = settings.harness.address.trim().to_string();
        if address.is_empty() {
            return all_services(Tier::Static, Outcome::Disabled, Reason::Disabled, "");
        }
        let started = std::time::Instant::now();
        let result = match crate::providers::service_harness::legacy_dynamic_lan_host(&address) {
            Ok(Some(host)) => fetch_catalog(&host, &settings.harness.larm_profile)
                .await
                .map(|advertised| {
                    let names: Vec<&str> = advertised.iter().map(String::as_str).collect();
                    from_advertised(&names)
                }),
            Ok(None) => crate::providers::service_harness::resolve(&address)
                .await
                .map(|resolution| from_resolution(&resolution, Tier::Static, &[])),
            Err(error) => Err(error),
        };
        let elapsed = started.elapsed().as_millis() as u64;
        match result {
            Ok(items) => items,
            Err(error) => all_services(
                Tier::Static,
                Outcome::Fail,
                classify_failure(&error),
                &error,
            )
            .into_iter()
            .map(|item| item.latency(Some(elapsed)))
            .collect(),
        }
    })
}

async fn fetch_catalog(host: &str, profile: &Option<String>) -> Result<Vec<String>, String> {
    let base = crate::providers::dynamic_lan::control_base_url(host)
        .map_err(|error| error.public_message().to_string())?;
    let credential = crate::providers::dynamic_lan::credential::load()
        .map_err(|error| error.code().to_string())?;
    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(3))
        .timeout(Duration::from_secs(5))
        .no_proxy()
        .build()
        .map_err(|_| "Could not initialize the LARM client".to_string())?;
    let selector = match crate::providers::larm_resources::profile::preference(profile.as_deref()) {
        saaa_larm_session::ProfilePreference::Variant(variant) => variant.selector().to_string(),
        saaa_larm_session::ProfilePreference::Explicit(value) => value,
    };
    let catalog = saaa_larm_session::catalog::fetch(&client, &base, credential.token(), &selector)
        .await
        .map_err(str::to_string)?;
    Ok(catalog
        .providers
        .iter()
        .map(|provider| provider.name.to_string())
        .collect())
}

pub(super) fn session(state: &AppState) -> CheckFuture<'_> {
    Box::pin(async move {
        let Ok(settings) = state.sqlite_readers.read(load_model_providers) else {
            return Vec::new();
        };
        let address = settings.harness.address.trim().to_string();
        if address.is_empty() {
            return Vec::new();
        }
        let started = std::time::Instant::now();
        match crate::providers::service_harness::resolve_for_diagnosis(
            &address,
            settings.harness.larm_profile.as_deref(),
        )
        .await
        {
            Ok(resolution) => {
                let latency = resolution
                    .diagnosis_timings
                    .as_ref()
                    .map(|timings| timings.provider_ms.clone())
                    .unwrap_or_default();
                from_resolution(&resolution, Tier::Probe, &latency)
            }
            Err(error) => {
                let mut reason = classify_failure(&error);
                if reason == Reason::Unavailable {
                    reason = refine_by_tcp(&address).await.unwrap_or(reason);
                }
                let elapsed = started.elapsed().as_millis() as u64;
                all_services(Tier::Probe, Outcome::Fail, reason, &error)
                    .into_iter()
                    .map(|item| item.latency(Some(elapsed)))
                    .collect()
            }
        }
    })
}

/// A generic failure on a LARM address whose control port is closed is an unreachable host.
async fn refine_by_tcp(address: &str) -> Option<Reason> {
    let host = crate::providers::service_harness::legacy_dynamic_lan_host(address)
        .ok()
        .flatten()?;
    let reached = tokio::time::timeout(
        Duration::from_secs(3),
        tokio::net::TcpStream::connect((
            host.as_str(),
            crate::providers::dynamic_lan::CONTROL_PORT,
        )),
    )
    .await
    .is_ok_and(|result| result.is_ok());
    (!reached).then_some(Reason::Unreachable)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn status(
        capability: &'static str,
        state: &'static str,
        message: &str,
    ) -> HarnessServiceStatus {
        HarnessServiceStatus {
            capability,
            state,
            protocol: None,
            model: None,
            language: None,
            voice: None,
            message: message.to_string(),
        }
    }

    fn resolution(services: Vec<HarnessServiceStatus>) -> HarnessResolution {
        HarnessResolution {
            state: "ready",
            revision: "r".into(),
            services,
            diagnosis_timings: None,
        }
    }

    fn find<'a>(items: &'a [Evidence], source: &str) -> &'a Evidence {
        items.iter().find(|item| item.source == source).unwrap()
    }

    #[test]
    fn services_map_to_the_capability_they_serve() {
        let items = from_advertised(&["llm", "asr", "tts", "embedding", "backchannel"]);
        assert_eq!(
            find(&items, "larm.llm").capability,
            Capability::Conversation
        );
        assert_eq!(find(&items, "larm.asr").capability, Capability::VoiceListen);
        assert_eq!(find(&items, "larm.tts").capability, Capability::VoiceSpeak);
        assert_eq!(
            find(&items, "larm.embedding").capability,
            Capability::Memory
        );
        assert_eq!(
            find(&items, "larm.embedding").importance,
            Importance::Advisory
        );
        assert!(items.iter().all(|item| item.route == "larm"));
    }

    #[test]
    fn advertised_services_are_static_evidence_never_proof() {
        let items = from_advertised(&["llm", "tts"]);
        assert!(items.iter().all(|item| item.tier == Tier::Static));
        assert_eq!(find(&items, "larm.llm").outcome, Outcome::Pass);
        let asr = find(&items, "larm.asr");
        assert_eq!(
            (asr.outcome, asr.reason),
            (Outcome::Fail, Reason::NotAdvertised)
        );
    }

    #[test]
    fn session_states_map_to_typed_outcomes() {
        let items = from_resolution(
            &resolution(vec![
                status("llm", "ready", ""),
                status("asr", "degraded", "warming up"),
                status("tts", "unavailable", "connection refused"),
                status("embedding", "unavailable", "Provider not advertised"),
            ]),
            Tier::Probe,
            &[("llm", 42)],
        );
        let llm = find(&items, "larm.llm");
        assert_eq!((llm.outcome, llm.latency_ms), (Outcome::Pass, Some(42)));
        assert_eq!(find(&items, "larm.asr").outcome, Outcome::Degraded);
        let tts = find(&items, "larm.tts");
        assert_eq!(
            (tts.outcome, tts.reason),
            (Outcome::Fail, Reason::Unreachable)
        );
        assert_eq!(find(&items, "larm.embedding").reason, Reason::NotAdvertised);
        // An optional service the link does not offer is disabled, not a fault.
        assert_eq!(find(&items, "larm.backchannel").outcome, Outcome::Disabled);
        assert!(items.iter().all(|item| item.tier == Tier::Probe));
    }

    #[test]
    fn a_missing_required_service_fails_but_a_missing_optional_one_does_not() {
        let items = from_resolution(
            &resolution(vec![status("llm", "ready", "")]),
            Tier::Probe,
            &[],
        );
        assert_eq!(find(&items, "larm.asr").outcome, Outcome::Fail);
        assert_eq!(find(&items, "larm.tts").reason, Reason::NotAdvertised);
        assert_eq!(find(&items, "larm.embedding").outcome, Outcome::Disabled);
    }

    #[test]
    fn a_failed_connection_fails_every_service_with_one_cause() {
        let items = all_services(Tier::Probe, Outcome::Fail, Reason::Timeout, "timed out");
        assert_eq!(items.len(), SERVICES.len());
        assert!(items.iter().all(|item| item.reason == Reason::Timeout));
    }

    #[tokio::test]
    async fn an_empty_address_marks_the_link_disabled_without_network_access() {
        let connection = rusqlite::Connection::open_in_memory().expect("database opens");
        crate::initialize_database(&connection).expect("database initializes");
        let state = crate::test_support::app_state(connection);
        let mut settings = state.sqlite_readers.read(load_model_providers).unwrap();
        settings.harness.address.clear();
        let value = serde_json::to_string(&settings).unwrap();
        state
            .sqlite_writer
            .write(move |connection| {
                connection
                    .execute(
                        "UPDATE settings_documents SET value_json=?1
                         WHERE namespace='providers.model' AND key='default'",
                        [value],
                    )
                    .map_err(crate::database_error)?;
                Ok(())
            })
            .unwrap();
        let items = catalog(&state).await;
        assert_eq!(items.len(), SERVICES.len());
        assert!(items.iter().all(|item| item.outcome == Outcome::Disabled));
        assert!(session(&state).await.is_empty());
    }
}
