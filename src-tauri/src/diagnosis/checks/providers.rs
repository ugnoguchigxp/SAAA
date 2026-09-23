use super::item;
use crate::diagnosis::contract::{DiagnosisItem, DiagnosisSeverity, DiagnosisStatus};
use crate::persistence::load_model_providers;
use crate::{AppState, ModelProviderSettings, TestProviderInput};
use std::time::Duration;

/// Flip to skip billable Cloud TTS probes. Startup and rerun share this check.
const SKIP_CLOUD_TTS_PROBE: bool = false;

pub(in crate::diagnosis) fn enabled_providers(state: &AppState) -> Vec<ModelProviderSettings> {
    let settings = match state.sqlite_readers.read(load_model_providers) {
        Ok(settings) => settings,
        Err(_) => return Vec::new(),
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

pub(in crate::diagnosis) async fn probe_one(
    state: &AppState,
    provider: &ModelProviderSettings,
) -> Vec<DiagnosisItem> {
    vec![probe_provider(state, provider).await]
}

pub(in crate::diagnosis) async fn codex_item(state: &AppState) -> Option<DiagnosisItem> {
    codex_sdk_item(state).await
}

#[cfg(test)]
pub(in crate::diagnosis) async fn providers(state: &AppState) -> Vec<DiagnosisItem> {
    let enabled = enabled_providers(state);
    let mut items = Vec::new();
    let mut pending = probe_tasks(state, &enabled);
    while !pending.is_empty() {
        let (item, _index, rest) = futures_util::future::select_all(pending).await;
        pending = rest;
        if let Some(item) = item {
            items.push(item);
        }
    }
    items
}

#[cfg(test)]
fn probe_tasks<'a>(
    state: &'a AppState,
    enabled: &'a [ModelProviderSettings],
) -> Vec<std::pin::Pin<Box<dyn std::future::Future<Output = Option<DiagnosisItem>> + 'a>>> {
    let mut pending = enabled
        .iter()
        .map(|provider| {
            Box::pin(async move { Some(probe_provider(state, provider).await) })
                as std::pin::Pin<Box<dyn std::future::Future<Output = Option<DiagnosisItem>> + 'a>>
        })
        .collect::<Vec<_>>();
    pending.push(Box::pin(async move { codex_sdk_item(state).await }));
    pending
}

async fn codex_sdk_item(state: &AppState) -> Option<DiagnosisItem> {
    let settings = state
        .sqlite_readers
        .read(crate::persistence::load_codex_settings)
        .ok()?;
    if !settings.enabled {
        return None;
    }
    let detail = match tokio::task::spawn_blocking(probe_codex_sdk).await {
        Ok(Ok(())) => (DiagnosisStatus::Ok, format!("{} responded", settings.model)),
        Ok(Err(error)) => (DiagnosisStatus::Fail, error),
        Err(_) => (DiagnosisStatus::Fail, "check failed".to_string()),
    };
    Some(item(
        "provider.codex-sdk",
        "llm",
        "Codex SDK",
        detail.0,
        DiagnosisSeverity::Degraded,
        &format!("Codex SDK: {}", detail.1),
        None,
    ))
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
    match crate::providers::probe::test_model_provider(
        state,
        TestProviderInput {
            provider: provider.clone(),
        },
    )
    .await
    {
        Ok(tested) => item(
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
        Err(error) => item(
            &id,
            group,
            provider.label(),
            DiagnosisStatus::Fail,
            DiagnosisSeverity::Degraded,
            &format!("{}: {error}", provider.label()),
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

    #[tokio::test]
    async fn enabled_codex_sdk_is_probed() {
        let _lock = crate::test_environment::codex_lock()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let directory = tempfile::tempdir().expect("temp dir");
        let script = directory.path().join("codex");
        std::fs::write(
            &script,
            "#!/usr/bin/env python3\nimport sys\nsys.stdin.readline()\nprint('{\"id\":1,\"result\":{}}', flush=True)\n",
        )
        .expect("script");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755))
                .expect("mode");
        }
        let _env = crate::test_environment::EnvGuard::set("SAAA_CODEX_PATH", &script);
        let state = fresh();
        let mut settings = loaded(&state);
        settings.providers.clear();
        write_settings(&state, &settings);
        write_codex(&state, true, "unchecked");
        let items = providers(&state).await;
        let codex = items
            .iter()
            .find(|item| item.id == "provider.codex-sdk")
            .expect("codex");
        assert_eq!(codex.group, "llm");
        assert_eq!(codex.status, DiagnosisStatus::Ok);
        assert!(!codex.message.contains("not checked"));
    }

    #[tokio::test]
    async fn disabled_codex_sdk_is_omitted() {
        let state = fresh();
        let mut settings = loaded(&state);
        settings
            .providers
            .retain(|provider| matches!(provider, ModelProviderSettings::SystemTts(_)));
        write_settings(&state, &settings);
        write_codex(&state, false, "ready");
        let items = providers(&state).await;
        assert!(items.iter().all(|item| item.id != "provider.codex-sdk"));
    }

    fn write_codex(state: &AppState, enabled: bool, health: &str) {
        let mut settings = state
            .sqlite_readers
            .read(crate::persistence::load_codex_settings)
            .expect("codex loads");
        settings.enabled = enabled;
        settings.health = health.to_string();
        let value = serde_json::to_string(&settings).expect("codex encodes");
        state
            .sqlite_writer
            .write(|connection| {
                connection
                    .execute(
                        "UPDATE settings_documents SET value_json=?1
                         WHERE namespace='providers.agent' AND key='codex-sdk'",
                        [value],
                    )
                    .map_err(crate::database_error)?;
                Ok(())
            })
            .expect("codex update");
    }
}
