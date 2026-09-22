use super::*;
use std::sync::atomic::{AtomicBool, Ordering};

pub(super) async fn render(
    receiver: &mut mpsc::Receiver<SpeechWork>,
    context: &RenderSessionContext,
) -> Result<(), String> {
    let TtsRoute::Fallback(routes, attempt_ms) = &context.route else {
        unreachable!()
    };
    let output = Arc::new(AtomicBool::new(false));
    let mut selected = 0;
    let mut first_phrase = true;
    loop {
        let work = tokio::select! {biased; _=context.cancellation.cancelled()=>return Ok(()), work=receiver.recv()=>work};
        let Some(SpeechWork::Chunk {
            text,
            expression,
            boundary_at,
        }) = work
        else {
            return Ok(());
        };
        let deadline = tokio::time::Instant::now() + Duration::from_millis(context.timeout_ms);
        loop {
            let route = routes
                .get(selected)
                .ok_or("TTS fallback routes exhausted")?;
            let remaining = deadline
                .saturating_duration_since(tokio::time::Instant::now())
                .as_millis() as u64;
            if remaining == 0 {
                return Err("TTS request timed out".into());
            }
            let budget = remaining.min(*attempt_ms);
            let result = tokio::select! {biased;
                _=context.cancellation.cancelled()=>return Ok(()),
                result=tokio::time::timeout(Duration::from_millis(budget), play_one(route, &text, boundary_at, budget, context, output.clone(), first_phrase))=>result.unwrap_or_else(|_|Err("TTS request timed out".into())),
            };
            match result {
                Ok(()) => {
                    first_phrase = false;
                    break;
                }
                Err(error)
                    if !output.load(Ordering::Acquire)
                        && crate::providers::route_policy::retryable(&error)
                        && selected + 1 < routes.len() =>
                {
                    selected += 1;
                    let _ = context.on_event.send(RuntimeEvent::Activity {
                        run_id: context.run_id.clone(),
                        kind: "tts-fallback".into(),
                        summary: format!(
                            "TTS switched to fallback {}: {}",
                            selected,
                            crate::providers::route_policy::failure_kind(&error).as_str()
                        ),
                    });
                }
                Err(error) => return Err(error),
            }
        }
    }
}
#[allow(clippy::too_many_arguments)]
async fn play_one(
    route: &TtsRoute,
    text: &str,
    boundary_at: Instant,
    budget: u64,
    context: &RenderSessionContext,
    output: Arc<AtomicBool>,
    first: bool,
) -> Result<(), String> {
    if crate::situation::speech_holds_runtime(&context.situation) {
        return Ok(());
    }
    let route = resolve_render_route(route, &context.cancellation).await?;
    let situation = context.situation.clone();
    let on_event = context.on_event.clone();
    let run_id = context.run_id.clone();
    let on_started = move || {
        if first {
            situation.set_audio_state(crate::situation::contracts::AudioState::SaaaSpeaking);
            let _ = on_event.send(RuntimeEvent::SpeechStarted { run_id });
        }
    };
    match route {
        TtsRoute::Cloud(provider) => {
            let provider = crate::voice::cloud_tts::speech_directive::apply_expression(
                &provider,
                expression,
            );
            crate::voice::http_audio::play_with_situation(
                &provider,
                text,
                budget,
                context.cancellation.clone(),
                output,
                on_started,
                Some(context.situation.clone()),
            )
            .await
        }
        TtsRoute::Larm(conversation, settings) => {
            let ready = crate::larm_voice::current_at(&conversation, &settings).await?;
            crate::voice::http_audio::play_larm_with_situation(
                &ready.session,
                settings.tts_voice.as_deref(),
                Some(&settings),
                expression,
                output,
                text,
                budget,
                context.cancellation.clone(),
                on_started,
                Some(context.situation.clone()),
            )
            .await
        }
        TtsRoute::System(provider) => {
            let chunk = render_future(
                TtsRoute::System(provider),
                0,
                text.to_string(),
                boundary_at,
                budget,
                context.cancellation.clone(),
                context.cache_directory.clone(),
            )
            .await?;
            if context.cancellation.is_cancelled()
                || crate::situation::speech_holds_runtime(&context.situation)
            {
                return Err("Speech cancelled".into());
            }
            // Mark before spawning: once playback can start it must never be retried.
            output.store(true, Ordering::Release);
            let child = crate::voice::cloud_tts::spawn_audio_player(&chunk.path)?;
            *context
                .child
                .lock()
                .map_err(|_| "Speech child lock unavailable")? = Some(child);
            on_started();
            let slot = context.child.clone();
            let cancellation = context.cancellation.clone();
            let result = tauri::async_runtime::spawn_blocking(move || {
                let result = wait_for_child(&slot, &cancellation);
                drop(chunk);
                result
            })
            .await
            .map_err(|_| "Speech worker stopped")??;
            if result.success() {
                Ok(())
            } else {
                Err("TTS playback failed".into())
            }
        }
        _ => Err("Unresolved TTS fallback".into()),
    }
}

#[cfg(test)]
#[path = "fallback_tests.rs"]
mod tests;
