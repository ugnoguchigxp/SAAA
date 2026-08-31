use std::{path::PathBuf, process::Child, sync::Arc, time::Duration};

use crate::{
    begin_simple_runtime_run, finish_runtime_run, register_active_run, remove_active_run,
    validate_identifier, ActiveTts, AppState, RunCancellation, SpeakTextInput,
};

#[derive(Clone)]
pub(crate) enum TtsRoute {
    Harness(String),
    Cloud(crate::CloudTtsProviderSettings),
    System(crate::SystemTtsProviderSettings),
}

pub(crate) async fn speak_text(state: &AppState, input: SpeakTextInput) -> Result<(), String> {
    validate_identifier(&input.run_id, "run id")?;
    validate_identifier(&input.conversation_id, "conversation id")?;
    if input.text.trim().is_empty() || input.text.chars().count() > 16_000 {
        return Err("Speech text must contain between 1 and 16,000 characters".to_string());
    }
    let speech_text = crate::voice_text::text_for_speech(&input.text);
    if speech_text.is_empty() {
        return Ok(());
    }
    let (route, provider_id, timeout_ms) = selected_tts_route(state)?;
    let cancellation = Arc::new(RunCancellation::default());
    register_active_run(state, &input.run_id, cancellation.clone())?;
    if let Err(error) = begin_simple_runtime_run(
        state,
        &input.run_id,
        &input.conversation_id,
        "voice.speak",
        &provider_id,
    ) {
        remove_active_run(state, &input.run_id);
        return Err(error);
    }
    let spawn_result =
        spawn_tts_route(state, route, &speech_text, timeout_ms, cancellation.clone()).await;
    let (child, artifact) = match spawn_result {
        Ok(spawned) => spawned,
        Err(error) => {
            remove_active_run(state, &input.run_id);
            let status = if cancellation.is_cancelled() {
                "cancelled"
            } else {
                "failed"
            };
            finish_runtime_run(state, &input.run_id, status, Some(&error))?;
            return Err(error);
        }
    };
    let active_tts = ActiveTts {
        run_id: input.run_id.clone(),
        child,
        artifact,
    };
    let install_result = match state.tts_process.lock() {
        Ok(mut process) if process.is_none() => {
            *process = Some(active_tts);
            Ok(())
        }
        Ok(_) => Err((
            active_tts,
            "Another speech run is already active".to_string(),
        )),
        Err(_) => Err((active_tts, "TTS process lock unavailable".to_string())),
    };
    if let Err((active_tts, error)) = install_result {
        cleanup_spawned_tts(active_tts.child, active_tts.artifact, true);
        remove_active_run(state, &input.run_id);
        finish_runtime_run(state, &input.run_id, "failed", Some(&error))?;
        return Err(error);
    }
    state
        .situation
        .set_audio_state(crate::situation::contracts::AudioState::SaaaSpeaking);
    let result = wait_for_tts(
        state,
        &input.run_id,
        cancellation.clone(),
        tts_run_timeout(&speech_text),
    )
    .await;
    cleanup_owned_tts(
        state,
        &input.run_id,
        cancellation.is_cancelled() || result.is_err(),
    );
    remove_active_run(state, &input.run_id);
    match result {
        Ok(()) => finish_runtime_run(state, &input.run_id, "completed", None),
        Err(error) if cancellation.is_cancelled() => {
            finish_runtime_run(state, &input.run_id, "cancelled", Some(&error))?;
            Err(error)
        }
        Err(error) => {
            finish_runtime_run(state, &input.run_id, "failed", Some(&error))?;
            Err(error)
        }
    }
}

pub(crate) fn selected_tts_route(state: &AppState) -> Result<(TtsRoute, String, u64), String> {
    let (providers, route) = state.sqlite_readers.read(|connection| {
        Ok((
            crate::persistence::load_model_providers(connection)?,
            crate::persistence::load_routing_settings(connection)?.voice_speak,
        ))
    })?;
    if route.source == "harness" {
        return Ok((
            TtsRoute::Harness(providers.harness.address),
            "provider-harness-tts".to_string(),
            route.timeout_ms,
        ));
    }
    let provider_id = route
        .provider_id
        .as_deref()
        .ok_or_else(|| "TTS provider is not selected".to_string())?;
    let selected = providers
        .providers
        .into_iter()
        .find_map(|provider| match provider {
            crate::ModelProviderSettings::CloudTts(provider)
                if provider.id == provider_id && provider.enabled =>
            {
                Some(TtsRoute::Cloud(provider))
            }
            crate::ModelProviderSettings::SystemTts(provider)
                if provider.id == provider_id && provider.enabled =>
            {
                Some(TtsRoute::System(provider))
            }
            _ => None,
        })
        .ok_or_else(|| "The selected TTS provider is unavailable".to_string())?;
    Ok((selected, provider_id.to_string(), route.timeout_ms))
}

async fn spawn_tts_route(
    state: &AppState,
    route: TtsRoute,
    text: &str,
    timeout_ms: u64,
    cancellation: Arc<RunCancellation>,
) -> Result<(Child, Option<PathBuf>), String> {
    if state.meeting.blocks_tts() {
        return Err("MEETING_POLICY_TTS_BLOCKED: Speech is disabled during a meeting.".to_string());
    }
    if state
        .tts_process
        .lock()
        .map(|value| value.is_some())
        .unwrap_or(true)
    {
        return Err("Another speech run is already active".to_string());
    }
    if cancellation.is_cancelled() {
        return Err("Speech cancelled".to_string());
    }
    match route {
        TtsRoute::System(provider) => {
            let spawned = crate::voice::system_tts::spawn_tts_process(text, &provider.voice)?;
            Ok((spawned.child, None))
        }
        TtsRoute::Cloud(provider) => {
            let (child, path) = crate::voice::cloud_tts::synthesize_to_player(
                &provider,
                text,
                timeout_ms,
                cancellation,
                &state.data_directory.join("tts-cache"),
            )
            .await?;
            Ok((child, Some(path)))
        }
        TtsRoute::Harness(address) => {
            let service = crate::providers::service_harness::resolve_service_cancellable(
                &address,
                "tts",
                &cancellation,
            )
            .await?;
            let provider = crate::CloudTtsProviderSettings {
                id: "provider-harness-tts".to_string(),
                enabled: true,
                label: "Provider Harness TTS".to_string(),
                location: "local".to_string(),
                endpoint: service.base_url,
                model: service.model,
                voice: service.voice.ok_or_else(|| {
                    "Provider Harness TTS descriptor does not include a voice".to_string()
                })?,
                authentication: "none".to_string(),
            };
            let (child, path) = crate::voice::cloud_tts::synthesize_to_player(
                &provider,
                text,
                timeout_ms,
                cancellation,
                &state.data_directory.join("tts-cache"),
            )
            .await?;
            Ok((child, Some(path)))
        }
    }
}

async fn wait_for_tts(
    state: &AppState,
    run_id: &str,
    cancellation: Arc<RunCancellation>,
    timeout: Duration,
) -> Result<(), String> {
    tokio::time::timeout(timeout, async {
        loop {
            if cancellation.is_cancelled() {
                return Err("Speech cancelled".to_string());
            }
            let status = {
                let mut process = state
                    .tts_process
                    .lock()
                    .map_err(|_| "TTS process lock unavailable".to_string())?;
                let active = process
                    .as_mut()
                    .filter(|active| active.run_id == run_id)
                    .ok_or_else(|| "Speech process ownership was lost".to_string())?;
                active
                    .child
                    .try_wait()
                    .map_err(|_| "Could not inspect TTS playback".to_string())?
            };
            if let Some(status) = status {
                return status
                    .success()
                    .then_some(())
                    .ok_or_else(|| format!("TTS playback exited with {status}"));
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .map_err(|_| "TTS playback reached its timeout".to_string())?
}

fn cleanup_spawned_tts(mut child: Child, artifact: Option<PathBuf>, kill: bool) {
    if kill {
        let _ = child.kill();
    }
    let _ = child.wait();
    if let Some(path) = artifact {
        let _ = std::fs::remove_file(path);
    }
}

fn cleanup_owned_tts(state: &AppState, run_id: &str, kill: bool) {
    if let Ok(mut process) = state.tts_process.lock() {
        if process
            .as_ref()
            .is_some_and(|active| active.run_id == run_id)
        {
            if let Some(active) = process.take() {
                cleanup_spawned_tts(active.child, active.artifact, kill);
            }
        }
    }
    state
        .situation
        .set_audio_state(crate::situation::contracts::AudioState::Silent);
}

pub(crate) fn stop_tts(state: &AppState, run_id: String) -> Result<(), String> {
    validate_identifier(&run_id, "run id")?;
    if let Ok(active_runs) = state.active_runs.lock() {
        if let Some(cancellation) = active_runs.get(&run_id) {
            cancellation.cancel();
        }
    }
    state.streaming_tts.cancel(&run_id);
    cleanup_owned_tts(state, &run_id, true);
    Ok(())
}

fn tts_run_timeout(text: &str) -> Duration {
    let estimated_seconds = 45_u64.saturating_add((text.chars().count() as u64).div_ceil(3));
    Duration::from_secs(estimated_seconds.clamp(60, 30 * 60))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn speech_timeout_scales_with_text_and_remains_bounded() {
        assert_eq!(tts_run_timeout("short"), Duration::from_secs(60));
        assert_eq!(
            tts_run_timeout(&"a".repeat(16_000)),
            Duration::from_secs(1_800)
        );
    }
}
