//! Model, ASR and TTS providers and the Codex SDK. Each configured provider is its own route,
//! so one working provider keeps the capability available.
use super::{classify_failure, evidence, pass, CheckFuture};
use crate::diagnosis::contract::{Capability, Evidence, Outcome, Reason, Tier};
use crate::persistence::load_model_providers;
use crate::{AppState, ModelProviderSettings, TestProviderInput};
use std::time::Duration;

const PROVIDER_PROBE_LIMIT: Duration = Duration::from_secs(45);

fn capability_of(provider: &ModelProviderSettings) -> Capability {
    match provider {
        ModelProviderSettings::CloudAsr(_) => Capability::VoiceListen,
        ModelProviderSettings::CloudTts(_) | ModelProviderSettings::SystemTts(_) => {
            Capability::VoiceSpeak
        }
        _ => Capability::Conversation,
    }
}

fn route_of(provider: &ModelProviderSettings) -> String {
    format!("provider:{}", provider.id())
}

/// Enabled providers, excluding the legacy LARM host entry that the LARM session covers.
fn enabled_providers(state: &AppState) -> Vec<ModelProviderSettings> {
    let Ok(settings) = state.sqlite_readers.read(load_model_providers) else {
        return Vec::new();
    };
    let legacy_host =
        crate::providers::service_harness::legacy_dynamic_lan_host(&settings.harness.address)
            .ok()
            .flatten();
    settings
        .providers
        .into_iter()
        .filter(|provider| {
            if let (Some(host), ModelProviderSettings::DynamicLan(settings)) =
                (&legacy_host, provider)
            {
                if settings.enabled && settings.host == *host {
                    return false;
                }
            }
            provider.enabled()
        })
        .collect()
}

fn codex_enabled(state: &AppState) -> Option<String> {
    state
        .sqlite_readers
        .read(crate::persistence::load_codex_settings)
        .ok()
        .filter(|settings| settings.enabled)
        .map(|settings| settings.model)
}

pub(super) fn config(state: &AppState) -> CheckFuture<'_> {
    Box::pin(async move {
        let providers = enabled_providers(state);
        let codex = codex_enabled(state);
        let harness_configured = state
            .sqlite_readers
            .read(load_model_providers)
            .is_ok_and(|settings| !settings.harness.address.trim().is_empty());
        let mut out: Vec<Evidence> = providers
            .iter()
            .map(|provider| {
                pass(
                    &format!("provider.{}", provider.id()),
                    capability_of(provider),
                    Tier::Static,
                )
                .route(route_of(provider))
                .subject(provider.label())
            })
            .collect();
        if let Some(model) = &codex {
            out.push(
                pass("codex.config", Capability::Conversation, Tier::Static)
                    .route("codex")
                    .detail(model),
            );
        }
        let has_reasoning = providers
            .iter()
            .any(|provider| capability_of(provider) == Capability::Conversation);
        if !has_reasoning && codex.is_none() && !harness_configured {
            out.push(evidence(
                "conversation.sources",
                Capability::Conversation,
                Tier::Static,
                Outcome::Fail,
                Reason::NotConfigured,
            ));
        }
        out
    })
}

pub(super) fn probe(state: &AppState) -> CheckFuture<'_> {
    Box::pin(async move {
        // A retest of one capability must not bill providers that serve another.
        let focus = state.diagnosis.focus();
        let providers: Vec<_> = enabled_providers(state)
            .into_iter()
            .filter(|provider| focus.is_none_or(|capability| capability_of(provider) == capability))
            .collect();
        futures_util::future::join_all(providers.iter().map(|provider| probe_one(state, provider)))
            .await
    })
}

async fn probe_one(state: &AppState, provider: &ModelProviderSettings) -> Evidence {
    // Distinct from the configuration row, which shares the provider id.
    let source = format!("provider.{}.availability", provider.id());
    let capability = capability_of(provider);
    let outcome = tokio::time::timeout(
        PROVIDER_PROBE_LIMIT,
        crate::providers::probe::test_model_provider(
            state,
            TestProviderInput {
                provider: provider.clone(),
            },
        ),
    )
    .await;
    // System TTS only reports availability without speaking, so it is not proof of output.
    let tier = if matches!(provider, ModelProviderSettings::SystemTts(_)) {
        Tier::Static
    } else {
        Tier::Probe
    };
    let item = match outcome {
        Ok(Ok(tested)) if tested.ok => {
            pass(&source, capability, tier).latency(u64::try_from(tested.latency_ms).ok())
        }
        Ok(Ok(tested)) => evidence(
            &source,
            capability,
            tier,
            Outcome::Fail,
            classify_failure(&tested.message),
        )
        .detail(&tested.message)
        .latency(u64::try_from(tested.latency_ms).ok()),
        Ok(Err(error)) => evidence(
            &source,
            capability,
            tier,
            Outcome::Fail,
            classify_failure(&error),
        )
        .detail(&error),
        Err(_) => evidence(&source, capability, tier, Outcome::Fail, Reason::Timeout),
    };
    item.route(route_of(provider)).subject(provider.label())
}

pub(super) fn codex(state: &AppState) -> CheckFuture<'_> {
    Box::pin(async move {
        let Some(model) = codex_enabled(state) else {
            return Vec::new();
        };
        // The handshake proves the process starts, not that a model answers: static evidence.
        if state
            .diagnosis
            .focus()
            .is_some_and(|capability| capability != Capability::Conversation)
        {
            return Vec::new();
        }
        let started = std::time::Instant::now();
        let item = match tokio::task::spawn_blocking(probe_codex_sdk).await {
            Ok(Ok(())) => pass("codex.sdk", Capability::Conversation, Tier::Static).detail(&model),
            Ok(Err(error)) => evidence(
                "codex.sdk",
                Capability::Conversation,
                Tier::Static,
                Outcome::Fail,
                classify_failure(&error),
            )
            .detail(&error),
            Err(_) => evidence(
                "codex.sdk",
                Capability::Conversation,
                Tier::Static,
                Outcome::Fail,
                Reason::Internal,
            ),
        };
        vec![item
            .route("codex")
            .latency(Some(started.elapsed().as_millis() as u64))]
    })
}

fn probe_codex_sdk() -> Result<(), String> {
    let mut child = crate::runtime::codex_cli::spawn_codex_app_server()?;
    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| "Codex SDK stdin is unavailable".to_string())?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "Codex SDK stdout is unavailable".to_string())?;
    let (sender, receiver) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let outcome = (|| {
            crate::runtime::codex_cli::write_codex_handshake(&mut stdin)?;
            let mut line = String::new();
            std::io::BufRead::read_line(&mut std::io::BufReader::new(stdout), &mut line)
                .map_err(|error| format!("Codex SDK did not respond: {error}"))?;
            let value: serde_json::Value = serde_json::from_str(line.trim())
                .map_err(|_| "Codex SDK returned invalid handshake output".to_string())?;
            if value.get("id").and_then(serde_json::Value::as_i64) == Some(1)
                && value.get("result").is_some()
            {
                return Ok(());
            }
            Err(value["error"]["message"]
                .as_str()
                .unwrap_or("Codex SDK rejected the handshake")
                .to_string())
        })();
        let _ = sender.send(outcome);
    });
    let outcome = match receiver.recv_timeout(Duration::from_secs(8)) {
        Ok(result) => result,
        Err(_) => Err("timed out after 8s".to_string()),
    };
    let _ = child.kill();
    let _ = child.wait();
    outcome
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;
    use std::time::Instant;

    fn fresh() -> AppState {
        let connection = Connection::open_in_memory().expect("database opens");
        crate::initialize_database(&connection).expect("database initializes");
        crate::test_support::app_state(connection)
    }

    fn configure(state: &AppState, with: impl FnOnce(&mut crate::ModelProvidersSettings)) {
        let mut settings = state
            .sqlite_readers
            .read(load_model_providers)
            .expect("providers load");
        with(&mut settings);
        let value = serde_json::to_string(&settings).expect("settings encode");
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
            .expect("settings update");
    }

    fn unreachable_provider(id: &str) -> ModelProviderSettings {
        let mut provider = crate::test_support::direct_provider(id, "local");
        provider.endpoint = "http://127.0.0.1:9/v1".to_string();
        ModelProviderSettings::OpenAiCompatible(provider)
    }

    #[tokio::test]
    async fn config_reports_each_enabled_provider_as_its_own_route() {
        let state = fresh();
        configure(&state, |settings| {
            settings.harness.address.clear();
            settings.providers = vec![unreachable_provider("local-llm")];
        });
        let items = config(&state).await;
        let item = items
            .iter()
            .find(|item| item.source == "provider.local-llm")
            .expect("provider is represented");
        assert_eq!(item.route, "provider:local-llm");
        assert_eq!(item.tier, Tier::Static);
        assert_eq!(item.capability, Capability::Conversation);
    }

    #[tokio::test]
    async fn no_reasoning_source_at_all_is_not_configured() {
        let state = fresh();
        configure(&state, |settings| {
            settings.harness.address.clear();
            settings.providers.clear();
        });
        let items = config(&state).await;
        let missing = items
            .iter()
            .find(|item| item.source == "conversation.sources")
            .expect("missing sources are reported");
        assert_eq!(
            (missing.outcome, missing.reason),
            (Outcome::Fail, Reason::NotConfigured)
        );
    }

    #[tokio::test]
    async fn probe_failure_is_typed_and_scoped_to_its_provider() {
        let state = fresh();
        configure(&state, |settings| {
            settings.harness.address.clear();
            settings.providers = vec![unreachable_provider("local-llm")];
        });
        let started = Instant::now();
        let items = probe(&state).await;
        assert!(started.elapsed() < Duration::from_secs(10));
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].outcome, Outcome::Fail);
        assert_eq!(items[0].tier, Tier::Probe);
        assert_eq!(items[0].route, "provider:local-llm");
        assert_ne!(items[0].reason, Reason::Ok);
    }
}
