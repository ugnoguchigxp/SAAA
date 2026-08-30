use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
    process::Child,
    sync::{Arc, Mutex},
    time::Duration,
};

use tokio::sync::mpsc;

use crate::{
    ipc_contract::RuntimeEvent,
    voice::session::{selected_tts_route, TtsRoute},
    AppState, RunCancellation,
};

use super::chunker::{SelectReason, SentenceAccumulator};

const MAX_QUEUED_CHUNKS: usize = 32;

/// Owns the sentence accumulator and a single serial renderer for every spoken turn.
/// It deliberately does not share the legacy one-shot `tts_process`: a streamed turn
/// can continue to accept deltas while the previous sentence is playing.
#[derive(Clone, Default)]
pub(crate) struct StreamingSpeechRuntime {
    sessions: Arc<Mutex<HashMap<String, SpeechSession>>>,
}

struct SpeechSession {
    accumulator: SentenceAccumulator,
    work: mpsc::Sender<SpeechWork>,
    cancellation: Arc<RunCancellation>,
    child: Arc<Mutex<Option<Child>>>,
    closed: bool,
    started: bool,
}

enum SpeechWork {
    Chunk(String),
    Finish,
}

struct RenderSessionContext {
    route: TtsRoute,
    timeout_ms: u64,
    cancellation: Arc<RunCancellation>,
    child: Arc<Mutex<Option<Child>>>,
    cache_directory: PathBuf,
    situation: Arc<crate::situation::SituationRuntime>,
    on_event: tauri::ipc::Channel<RuntimeEvent>,
    run_id: String,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct AppendOutcome {
    pub(crate) started: bool,
    pub(crate) idle_generation: u64,
}

impl StreamingSpeechRuntime {
    pub(crate) fn event_sink(
        &self,
        on_event: tauri::ipc::Channel<RuntimeEvent>,
        streaming_speech: bool,
    ) -> tauri::ipc::Channel<RuntimeEvent> {
        let speech_runtime = self.clone();
        tauri::ipc::Channel::new(move |body| {
            if let tauri::ipc::InvokeResponseBody::Json(value) = body {
                if let Ok(event) = serde_json::from_str::<RuntimeEvent>(&value) {
                    match &event {
                        RuntimeEvent::Delta { run_id, text } if streaming_speech => {
                            let outcome = match speech_runtime.append(run_id, text) {
                                Ok(outcome) => outcome,
                                Err(_) => {
                                    speech_runtime.cancel(run_id);
                                    Default::default()
                                }
                            };
                            if outcome.started {
                                let _ = on_event.send(RuntimeEvent::SpeechStarted {
                                    run_id: run_id.clone(),
                                });
                            }
                            let runtime_for_idle = speech_runtime.clone();
                            let event_for_idle = on_event.clone();
                            let idle_run_id = run_id.clone();
                            tauri::async_runtime::spawn(async move {
                                tokio::time::sleep(Duration::from_millis(400)).await;
                                if runtime_for_idle
                                    .flush_idle(&idle_run_id, outcome.idle_generation)
                                    .map(|outcome| outcome.started)
                                    .unwrap_or(false)
                                {
                                    let _ = event_for_idle.send(RuntimeEvent::SpeechStarted {
                                        run_id: idle_run_id,
                                    });
                                }
                            });
                        }
                        RuntimeEvent::MessageCompleted { run_id, message } if streaming_speech => {
                            if speech_runtime.finish(run_id, &message.content).is_err() {
                                speech_runtime.cancel(run_id);
                            }
                        }
                        RuntimeEvent::Cancelled { run_id }
                        | RuntimeEvent::Failed { run_id, .. }
                            if streaming_speech =>
                        {
                            speech_runtime.cancel(run_id);
                        }
                        _ => {}
                    }
                    let _ = on_event.send(event);
                }
            }
            Ok(())
        })
    }

    pub(crate) async fn begin(
        &self,
        state: &AppState,
        run_id: &str,
        on_event: tauri::ipc::Channel<RuntimeEvent>,
    ) -> Result<(), String> {
        if state.meeting.blocks_tts() {
            return Err(
                "MEETING_POLICY_TTS_BLOCKED: Speech is disabled during a meeting.".to_string(),
            );
        }
        let (route, _provider_id, timeout_ms) = selected_tts_route(state)?;
        let mut sessions = self
            .sessions
            .lock()
            .map_err(|_| "Streaming speech runtime lock unavailable".to_string())?;
        if sessions.contains_key(run_id) {
            return Err("A streamed speech session already exists for this run".to_string());
        }
        let (work, receiver) = mpsc::channel(MAX_QUEUED_CHUNKS);
        let cancellation = Arc::new(RunCancellation::default());
        let child = Arc::new(Mutex::new(None));
        sessions.insert(
            run_id.to_string(),
            SpeechSession {
                accumulator: SentenceAccumulator::default(),
                work,
                cancellation: cancellation.clone(),
                child: child.clone(),
                closed: false,
                started: false,
            },
        );
        drop(sessions);

        let sessions = self.sessions.clone();
        let run_id = run_id.to_string();
        let cache_directory = state.data_directory.join("tts-cache");
        let situation = state.situation.clone();
        tauri::async_runtime::spawn(async move {
            render_session(
                receiver,
                RenderSessionContext {
                    route,
                    timeout_ms,
                    cancellation,
                    child,
                    cache_directory,
                    situation: situation.clone(),
                    on_event: on_event.clone(),
                    run_id: run_id.clone(),
                },
            )
            .await;
            if let Ok(mut active) = sessions.lock() {
                active.remove(&run_id);
            }
            situation.set_audio_state(crate::situation::contracts::AudioState::Silent);
            let _ = on_event.send(RuntimeEvent::SpeechEnded { run_id });
        });
        Ok(())
    }

    pub(crate) fn append(&self, run_id: &str, delta: &str) -> Result<AppendOutcome, String> {
        let mut sessions = self
            .sessions
            .lock()
            .map_err(|_| "Streaming speech runtime lock unavailable".to_string())?;
        let Some(session) = sessions.get_mut(run_id) else {
            return Ok(AppendOutcome::default());
        };
        if session.closed || session.cancellation.is_cancelled() {
            return Ok(AppendOutcome::default());
        }
        session
            .accumulator
            .append(delta)
            .map_err(|error| format!("Invalid streamed speech input: {error:?}"))?;
        let mut queued = false;
        while let Some(chunk) = session.accumulator.next_chunk(SelectReason::Append) {
            session
                .work
                .try_send(SpeechWork::Chunk(chunk.spoken))
                .map_err(|_| {
                    "Speech playback cannot keep up with the response stream".to_string()
                })?;
            queued = true;
        }
        let started = queued && !session.started;
        session.started |= queued;
        Ok(AppendOutcome {
            started,
            idle_generation: session.accumulator.idle_generation(),
        })
    }

    pub(crate) fn flush_idle(
        &self,
        run_id: &str,
        expected_generation: u64,
    ) -> Result<AppendOutcome, String> {
        let mut sessions = self
            .sessions
            .lock()
            .map_err(|_| "Streaming speech runtime lock unavailable".to_string())?;
        let Some(session) = sessions.get_mut(run_id) else {
            return Ok(AppendOutcome::default());
        };
        if session.closed
            || session.cancellation.is_cancelled()
            || session.accumulator.idle_generation() != expected_generation
        {
            return Ok(AppendOutcome::default());
        }
        let mut queued = false;
        while let Some(chunk) = session.accumulator.next_chunk(SelectReason::Idle) {
            session
                .work
                .try_send(SpeechWork::Chunk(chunk.spoken))
                .map_err(|_| {
                    "Speech playback cannot keep up with the response stream".to_string()
                })?;
            queued = true;
        }
        let started = queued && !session.started;
        session.started |= queued;
        Ok(AppendOutcome {
            started,
            idle_generation: expected_generation,
        })
    }

    pub(crate) fn finish(&self, run_id: &str, final_content: &str) -> Result<(), String> {
        let mut sessions = self
            .sessions
            .lock()
            .map_err(|_| "Streaming speech runtime lock unavailable".to_string())?;
        let Some(session) = sessions.get_mut(run_id) else {
            return Ok(());
        };
        if session.closed {
            return Ok(());
        }
        session
            .accumulator
            .finish(final_content)
            .map_err(|error| format!("Invalid final streamed speech input: {error:?}"))?;
        while let Some(chunk) = session.accumulator.next_chunk(SelectReason::Completion) {
            session
                .work
                .try_send(SpeechWork::Chunk(chunk.spoken))
                .map_err(|_| {
                    "Speech playback cannot keep up with the response stream".to_string()
                })?;
        }
        session.closed = true;
        let work = session.work.clone();
        tauri::async_runtime::spawn(async move {
            let _ = work.send(SpeechWork::Finish).await;
        });
        Ok(())
    }

    pub(crate) fn cancel(&self, run_id: &str) {
        let session = self
            .sessions
            .lock()
            .ok()
            .and_then(|mut sessions| sessions.remove(run_id));
        if let Some(session) = session {
            session.cancellation.cancel();
            if let Ok(mut child) = session.child.lock() {
                if let Some(mut child) = child.take() {
                    let _ = child.kill();
                    let _ = child.wait();
                }
            }
        }
    }

    pub(crate) fn is_active(&self) -> bool {
        self.sessions
            .lock()
            .map(|sessions| !sessions.is_empty())
            .unwrap_or(true)
    }

    pub(crate) fn shutdown(&self) {
        let run_ids = self
            .sessions
            .lock()
            .map(|sessions| sessions.keys().cloned().collect::<Vec<_>>())
            .unwrap_or_default();
        for run_id in run_ids {
            self.cancel(&run_id);
        }
    }
}

async fn render_session(mut receiver: mpsc::Receiver<SpeechWork>, context: RenderSessionContext) {
    let mut speaking = false;
    while let Some(work) = receiver.recv().await {
        match work {
            SpeechWork::Finish => break,
            SpeechWork::Chunk(text) => {
                if context.cancellation.is_cancelled() {
                    break;
                }
                if !speaking {
                    context
                        .situation
                        .set_audio_state(crate::situation::contracts::AudioState::SaaaSpeaking);
                    speaking = true;
                }
                if let Err(error) = render_chunk(
                    &context.route,
                    &text,
                    context.timeout_ms,
                    context.cancellation.clone(),
                    &context.child,
                    &context.cache_directory,
                )
                .await
                {
                    if !context.cancellation.is_cancelled() {
                        let _ = context.on_event.send(RuntimeEvent::SpeechFailed {
                            run_id: context.run_id.clone(),
                            message: crate::redact_runtime_text(&error),
                            recovery: "Check the speech provider and try another response."
                                .to_string(),
                        });
                    }
                    break;
                }
            }
        }
    }
}

async fn render_chunk(
    route: &TtsRoute,
    text: &str,
    timeout_ms: u64,
    cancellation: Arc<RunCancellation>,
    child_slot: &Arc<Mutex<Option<Child>>>,
    cache_directory: &Path,
) -> Result<(), String> {
    let (mut child, artifact) = match route {
        TtsRoute::System(provider) => (
            crate::voice::system_tts::spawn_tts_process(text, &provider.voice)?.child,
            None,
        ),
        TtsRoute::Cloud(provider) => crate::voice::cloud_tts::synthesize_to_player(
            provider,
            text,
            timeout_ms,
            cancellation.clone(),
            cache_directory,
        )
        .await
        .map(|(child, path)| (child, Some(path)))?,
        TtsRoute::Harness(address) => {
            let service = crate::providers::service_harness::resolve_service_cancellable(
                address,
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
            crate::voice::cloud_tts::synthesize_to_player(
                &provider,
                text,
                timeout_ms,
                cancellation.clone(),
                cache_directory,
            )
            .await
            .map(|(child, path)| (child, Some(path)))?
        }
    };
    {
        let mut slot = child_slot
            .lock()
            .map_err(|_| "Streaming speech child lock unavailable".to_string())?;
        if cancellation.is_cancelled() {
            let _ = child.kill();
            let _ = child.wait();
            if let Some(path) = artifact {
                let _ = fs::remove_file(path);
            }
            return Ok(());
        }
        *slot = Some(child);
    }
    let status_result = tokio::task::spawn_blocking({
        let child_slot = child_slot.clone();
        let cancellation = cancellation.clone();
        move || wait_for_child(&child_slot, &cancellation)
    })
    .await
    .map_err(|_| "Streaming speech worker stopped unexpectedly".to_string())?;
    if let Ok(mut slot) = child_slot.lock() {
        let _ = slot.take();
    }
    if let Some(path) = artifact {
        let _ = fs::remove_file(path);
    }
    let status = match status_result {
        Ok(status) => status,
        Err(_) if cancellation.is_cancelled() => return Ok(()),
        Err(error) => return Err(error),
    };
    if status.success() || cancellation.is_cancelled() {
        Ok(())
    } else {
        Err(format!("TTS playback exited with {status}"))
    }
}

fn wait_for_child(
    child_slot: &Arc<Mutex<Option<Child>>>,
    cancellation: &RunCancellation,
) -> Result<std::process::ExitStatus, String> {
    loop {
        if cancellation.is_cancelled() {
            return Err("Speech cancelled".to_string());
        }
        let status = child_slot
            .lock()
            .map_err(|_| "Streaming speech child lock unavailable".to_string())?
            .as_mut()
            .ok_or_else(|| "Streaming speech child ownership was lost".to_string())?
            .try_wait()
            .map_err(|_| "Could not inspect streamed speech playback".to_string())?;
        if let Some(status) = status {
            return Ok(status);
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}
