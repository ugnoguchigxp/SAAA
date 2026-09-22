use super::item;
use crate::diagnosis::contract::{DiagnosisItem, DiagnosisSeverity, DiagnosisStatus};
use crate::persistence::load_model_providers;
use crate::{AppState, ModelProviderSettings, TestProviderInput};
use std::time::Duration;

/// Flip to skip billable Cloud TTS probes. Startup and rerun share this check.
const SKIP_CLOUD_TTS_PROBE: bool = false;

pub(in crate::diagnosis) async fn providers(state: &AppState) -> Vec<DiagnosisItem> {
    let settings = match state.sqlite_readers.read(load_model_providers) {
        Ok(settings) => settings,
        Err(_) => return Vec::new(),
    };
    let enabled = settings
        .providers
        .into_iter()
        .filter(|provider| provider.enabled())
        .collect::<Vec<_>>();
    let checks = enabled
        .iter()
        .map(|provider| probe_provider(state, provider))
        .collect::<Vec<_>>();
    futures_util::future::join_all(checks).await
}

async fn probe_provider(state: &AppState, provider: &ModelProviderSettings) -> DiagnosisItem {
    let id = format!("provider.{}", provider.id());
    let group = match provider {
        ModelProviderSettings::CloudAsr(_)
        | ModelProviderSettings::CloudTts(_)
        | ModelProviderSettings::SystemTts(_) => "voice",
        _ => "llm",
    };
    if SKIP_CLOUD_TTS_PROBE && matches!(provider, ModelProviderSettings::CloudTts(_)) {
        return item(
            &id,
            group,
            provider.label(),
            DiagnosisStatus::Skipped,
            DiagnosisSeverity::Degraded,
            &format!("{}: cloud TTS probe skipped", provider.label()),
            None,
        );
    }
    match tokio::time::timeout(
        Duration::from_secs(8),
        crate::providers::probe::test_model_provider(
            state,
            TestProviderInput {
                provider: provider.clone(),
            },
        ),
    )
    .await
    {
        Ok(Ok(tested)) => item(
            &id,
            group,
            provider.label(),
            if tested.ok {
                DiagnosisStatus::Ok
            } else {
                DiagnosisStatus::Fail
            },
            DiagnosisSeverity::Degraded,
            &format!("{}: {}", provider.label(), tested.message),
            u64::try_from(tested.latency_ms).ok(),
        ),
        Ok(Err(error)) => item(
            &id,
            group,
            provider.label(),
            DiagnosisStatus::Fail,
            DiagnosisSeverity::Degraded,
            &format!("{}: {error}", provider.label()),
            None,
        ),
        Err(_) => item(
            &id,
            group,
            provider.label(),
            DiagnosisStatus::Fail,
            DiagnosisSeverity::Degraded,
            &format!("{}: timed out after 8s", provider.label()),
            None,
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ModelProviderSettings;
    use rusqlite::Connection;
    use std::time::Instant;

    fn fresh() -> AppState {
        let connection = Connection::open_in_memory().expect("database opens");
        crate::initialize_database(&connection).expect("database initializes");
        crate::test_support::app_state(connection)
    }

    fn write_settings(state: &AppState, settings: &crate::ModelProvidersSettings) {
        let value = serde_json::to_string(settings).expect("settings encode");
        state
            .sqlite_writer
            .write(|connection| {
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

    fn loaded(state: &AppState) -> crate::ModelProvidersSettings {
        state
            .sqlite_readers
            .read(load_model_providers)
            .expect("providers load")
    }

    #[tokio::test]
    async fn dg_05_system_tts_reports_ok() {
        let state = fresh();
        let mut settings = loaded(&state);
        settings
            .providers
            .retain(|provider| matches!(provider, ModelProviderSettings::SystemTts(_)));
        write_settings(&state, &settings);
        let items = providers(&state).await;
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].status, DiagnosisStatus::Ok);
        assert_eq!(items[0].group, "voice");
        assert_eq!(items[0].id, "provider.system-tts");
    }

    #[tokio::test]
    async fn dg_05_disabled_provider_is_omitted() {
        let state = fresh();
        let mut settings = loaded(&state);
        for provider in &mut settings.providers {
            if !matches!(provider, ModelProviderSettings::SystemTts(_)) {
                provider.set_enabled(false);
            }
        }
        write_settings(&state, &settings);
        let items = providers(&state).await;
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].id, "provider.system-tts");
    }

    #[tokio::test]
    async fn dg_05_unreachable_provider_fails_within_timeout() {
        let state = fresh();
        let mut settings = loaded(&state);
        settings.providers.clear();
        settings
            .providers
            .push(crate::test_support::provider("remote", "cloud"));
        write_settings(&state, &settings);
        let started = Instant::now();
        let items = providers(&state).await;
        assert!(started.elapsed() < Duration::from_secs(10));
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].id, "provider.remote");
        assert_eq!(items[0].status, DiagnosisStatus::Fail);
        assert_eq!(items[0].group, "llm");
    }
}
