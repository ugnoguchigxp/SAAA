use std::{
    collections::{BTreeMap, HashMap, VecDeque},
    fs,
    future::Future,
    io::Read,
    path::{Path, PathBuf},
    pin::Pin,
    process::Child,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use futures_util::{stream::FuturesUnordered, StreamExt};
use tokio::sync::{mpsc, watch};

use crate::{
    ipc_contract::RuntimeEvent,
    voice::session::{selected_tts_route, TtsRoute},
    AppState, RunCancellation,
};

use super::chunker::{SelectReason, SentenceAccumulator};

const MAX_QUEUED_CHUNKS: usize = 32;
const MAX_RENDER_CONCURRENCY: usize = 3;
const MAX_READY_CHUNKS: usize = 3;
const MAX_READY_AUDIO_BYTES: u64 = 16 * 1_024 * 1_024;
const MAX_READY_AUDIO_MS: u64 = 30_000;

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
    enabled: bool,
    idle_timer: Option<tauri::async_runtime::JoinHandle<()>>,
    idle_reset: watch::Sender<Option<u64>>,
}

enum SpeechWork {
    Chunk { text: String, boundary_at: Instant },
    Finish,
}

fn queue_chunk(session: &SpeechSession, text: String) -> Result<(), String> {
    let boundary_at = Instant::now();
    session
        .work
        .try_send(SpeechWork::Chunk { text, boundary_at })
        .map_err(|_| "Speech playback cannot keep up with the response stream".to_string())?;
    crate::runtime::event_hub::performance::record_tts_boundary_to_dispatch(boundary_at.elapsed());
    Ok(())
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
    pub(crate) idle_generation: u64,
}

impl StreamingSpeechRuntime {
    pub(crate) async fn begin(
        &self,
        state: &AppState,
        run_id: &str,
        enabled: bool,
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
        let (idle_reset, mut idle_resets) = watch::channel(None::<u64>);
        let cancellation = Arc::new(RunCancellation::default());
        let child = Arc::new(Mutex::new(None));
        let idle_runtime = self.clone();
        let idle_run_id = run_id.to_string();
        let idle_timer = tauri::async_runtime::spawn(async move {
            while idle_resets.changed().await.is_ok() {
                let Some(mut generation) = *idle_resets.borrow_and_update() else {
                    break;
                };
                loop {
                    tokio::select! {
                        _ = tokio::time::sleep(Duration::from_millis(400)) => {
                            let _ = idle_runtime.flush_idle(&idle_run_id, generation);
                            break;
                        }
                        changed = idle_resets.changed() => {
                            if changed.is_err() {
                                return;
                            }
                            let Some(next) = *idle_resets.borrow_and_update() else {
                                return;
                            };
                            generation = next;
                        }
                    }
                }
            }
        });
        sessions.insert(
            run_id.to_string(),
            SpeechSession {
                accumulator: SentenceAccumulator::default(),
                work,
                cancellation: cancellation.clone(),
                child: child.clone(),
                closed: false,
                enabled,
                idle_timer: Some(idle_timer),
                idle_reset,
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
        if session.closed || session.cancellation.is_cancelled() || !session.enabled {
            return Ok(AppendOutcome::default());
        }
        session
            .accumulator
            .append(delta)
            .map_err(|error| format!("Invalid streamed speech input: {error:?}"))?;
        while let Some(chunk) = session.accumulator.next_chunk(SelectReason::Append) {
            queue_chunk(session, chunk.spoken)?;
        }
        Ok(AppendOutcome {
            idle_generation: session.accumulator.idle_generation(),
        })
    }

    pub(crate) fn schedule_idle(&self, run_id: &str, expected_generation: u64) {
        if let Ok(sessions) = self.sessions.lock() {
            if let Some(session) = sessions.get(run_id) {
                if !session.closed && !session.cancellation.is_cancelled() && session.enabled {
                    let _ = session.idle_reset.send(Some(expected_generation));
                }
            }
        }
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
            || !session.enabled
            || session.accumulator.idle_generation() != expected_generation
        {
            return Ok(AppendOutcome::default());
        }
        while let Some(chunk) = session.accumulator.next_chunk(SelectReason::Idle) {
            queue_chunk(session, chunk.spoken)?;
        }
        Ok(AppendOutcome {
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
        if session.closed || !session.enabled {
            return Ok(());
        }
        if let Some(timer) = session.idle_timer.take() {
            let _ = session.idle_reset.send(None);
            timer.abort();
        }
        session
            .accumulator
            .finish(final_content)
            .map_err(|error| format!("Invalid final streamed speech input: {error:?}"))?;
        while let Some(chunk) = session.accumulator.next_chunk(SelectReason::Completion) {
            queue_chunk(session, chunk.spoken)?;
        }
        session.closed = true;
        let work = session.work.clone();
        tauri::async_runtime::spawn(async move {
            let _ = work.send(SpeechWork::Finish).await;
        });
        Ok(())
    }

    pub(crate) fn set_enabled(&self, run_id: &str, enabled: bool) {
        if !enabled {
            self.cancel(run_id);
            return;
        }
        if let Ok(mut sessions) = self.sessions.lock() {
            if let Some(session) = sessions.get_mut(run_id) {
                session.enabled = true;
            }
        }
    }

    pub(crate) fn cancel(&self, run_id: &str) {
        let session = self
            .sessions
            .lock()
            .ok()
            .and_then(|mut sessions| sessions.remove(run_id));
        if let Some(session) = session {
            if let Some(timer) = session.idle_timer {
                let _ = session.idle_reset.send(None);
                timer.abort();
            }
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

struct RenderedChunk {
    sequence: u64,
    boundary_at: Instant,
    path: PathBuf,
    bytes: u64,
    audio_ms: u64,
    synthesis_ms: u64,
}

impl Drop for RenderedChunk {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

type RenderFuture = Pin<Box<dyn Future<Output = Result<RenderedChunk, String>> + Send>>;
type PlaybackTask =
    tauri::async_runtime::JoinHandle<(RenderedChunk, Result<std::process::ExitStatus, String>)>;

async fn render_session(
    mut receiver: mpsc::Receiver<SpeechWork>,
    mut context: RenderSessionContext,
) {
    let result = render_session_inner(&mut receiver, &mut context).await;
    if let Err(error) = result {
        let externally_cancelled = context.cancellation.is_cancelled();
        context.cancellation.cancel();
        if let Ok(mut child) = context.child.lock() {
            if let Some(mut child) = child.take() {
                let _ = child.kill();
                let _ = child.wait();
            }
        }
        if !externally_cancelled {
            let _ = context.on_event.send(RuntimeEvent::SpeechFailed {
                run_id: context.run_id.clone(),
                message: crate::redact_runtime_text(&error),
                recovery: "Check the speech provider and try another response.".to_string(),
            });
        }
    }
}

async fn render_session_inner(
    receiver: &mut mpsc::Receiver<SpeechWork>,
    context: &mut RenderSessionContext,
) -> Result<(), String> {
    context.route = resolve_render_route(&context.route, &context.cancellation).await?;
    let mut queued = VecDeque::<(u64, String, Instant)>::new();
    let mut rendering = FuturesUnordered::<RenderFuture>::new();
    let mut ready = BTreeMap::<u64, RenderedChunk>::new();
    let mut playback: Option<PlaybackTask> = None;
    let mut next_sequence = 0_u64;
    let mut next_playback = 0_u64;
    let mut input_closed = false;
    let mut playback_started = false;
    let mut adaptive_concurrency = 2_usize;
    let mut timing_samples = VecDeque::<(u64, u64)>::new();
    let mut ready_bytes = 0_u64;
    let mut ready_audio_ms = 0_u64;

    loop {
        crate::runtime::event_hub::performance::record_tts_queue_depth(render_slots_used(
            rendering.len(),
            ready.len(),
            playback.is_some(),
        ));
        if context.cancellation.is_cancelled() {
            break;
        }
        while rendering.len() < adaptive_concurrency
            && render_slots_used(rendering.len(), ready.len(), playback.is_some())
                < MAX_READY_CHUNKS
            && ready_bytes < MAX_READY_AUDIO_BYTES
            && ready_audio_ms < MAX_READY_AUDIO_MS
        {
            let Some((sequence, text, boundary_at)) = queued.pop_front() else {
                break;
            };
            rendering.push(render_future(
                context.route.clone(),
                sequence,
                text,
                boundary_at,
                context.timeout_ms,
                context.cancellation.clone(),
                context.cache_directory.clone(),
            ));
        }

        if playback.is_none() {
            if let Some(chunk) = ready.remove(&next_playback) {
                ready_bytes = ready_bytes.saturating_sub(chunk.bytes);
                ready_audio_ms = ready_audio_ms.saturating_sub(chunk.audio_ms);
                let mut child = crate::voice::cloud_tts::spawn_audio_player(&chunk.path)?;
                crate::runtime::event_hub::performance::record_tts_boundary_to_player_spawn(
                    chunk.boundary_at.elapsed(),
                );
                {
                    let mut slot = context
                        .child
                        .lock()
                        .map_err(|_| "Streaming speech child lock unavailable".to_string())?;
                    if context.cancellation.is_cancelled() {
                        let _ = child.kill();
                        let _ = child.wait();
                        let _ = fs::remove_file(&chunk.path);
                        break;
                    }
                    *slot = Some(child);
                }
                if !playback_started {
                    context
                        .situation
                        .set_audio_state(crate::situation::contracts::AudioState::SaaaSpeaking);
                    context
                        .on_event
                        .send(RuntimeEvent::SpeechStarted {
                            run_id: context.run_id.clone(),
                        })
                        .map_err(|_| "Speech event consumer disconnected".to_string())?;
                    playback_started = true;
                }
                let child_slot = context.child.clone();
                let cancellation = context.cancellation.clone();
                playback = Some(tauri::async_runtime::spawn_blocking(move || {
                    let status = wait_for_child(&child_slot, &cancellation);
                    (chunk, status)
                }));
            }
        }

        if input_closed
            && queued.is_empty()
            && rendering.is_empty()
            && ready.is_empty()
            && playback.is_none()
        {
            break;
        }

        tokio::select! {
            _ = context.cancellation.cancelled() => break,
            work = receiver.recv(), if !input_closed && queued.len() < MAX_QUEUED_CHUNKS => {
                match work {
                    Some(SpeechWork::Chunk { text, boundary_at }) => {
                        queued.push_back((next_sequence, text, boundary_at));
                        next_sequence += 1;
                    }
                    Some(SpeechWork::Finish) | None => input_closed = true,
                }
            }
            rendered = rendering.next(), if !rendering.is_empty() => {
                let rendered = rendered.expect("rendering is non-empty")?;
                if rendered.bytes > MAX_READY_AUDIO_BYTES
                    || rendered.audio_ms > MAX_READY_AUDIO_MS
                    || ready_bytes.saturating_add(rendered.bytes) > MAX_READY_AUDIO_BYTES
                    || ready_audio_ms.saturating_add(rendered.audio_ms) > MAX_READY_AUDIO_MS
                {
                    let _ = fs::remove_file(&rendered.path);
                    return Err("Rendered speech exceeded the bounded ready-audio budget".to_string());
                }
                ready_bytes += rendered.bytes;
                ready_audio_ms += rendered.audio_ms;
                timing_samples.push_back((rendered.synthesis_ms, rendered.audio_ms.max(1)));
                if timing_samples.len() > 8 {
                    timing_samples.pop_front();
                }
                adaptive_concurrency = adaptive_render_concurrency(&timing_samples);
                ready.insert(rendered.sequence, rendered);
            }
            completed = async {
                playback.as_mut().expect("playback is present").await
            }, if playback.is_some() => {
                let (chunk, status) = completed
                    .map_err(|_| "Streaming speech playback worker stopped unexpectedly".to_string())?;
                if let Ok(mut slot) = context.child.lock() {
                    let _ = slot.take();
                }
                let _ = fs::remove_file(&chunk.path);
                playback = None;
                next_playback += 1;
                match status {
                    Ok(status) if status.success() => {}
                    Ok(status) if context.cancellation.is_cancelled() => break,
                    Ok(status) => return Err(format!("TTS playback exited with {status}")),
                    Err(_) if context.cancellation.is_cancelled() => break,
                    Err(error) => return Err(error),
                }
            }
        }
    }

    for chunk in ready.into_values() {
        let _ = fs::remove_file(&chunk.path);
    }
    Ok(())
}

async fn resolve_render_route(
    route: &TtsRoute,
    cancellation: &RunCancellation,
) -> Result<TtsRoute, String> {
    match route {
        TtsRoute::Harness(address) => {
            let service = crate::providers::service_harness::resolve_service_cancellable(
                address,
                "tts",
                cancellation,
            )
            .await?;
            Ok(TtsRoute::Cloud(crate::CloudTtsProviderSettings {
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
            }))
        }
        route => Ok(route.clone()),
    }
}

fn render_future(
    route: TtsRoute,
    sequence: u64,
    text: String,
    boundary_at: Instant,
    timeout_ms: u64,
    cancellation: Arc<RunCancellation>,
    cache_directory: PathBuf,
) -> RenderFuture {
    Box::pin(async move {
        let started = Instant::now();
        let path = match route {
            TtsRoute::System(provider) => {
                crate::voice::system_tts::render_tts_artifact(
                    text,
                    provider.voice,
                    cache_directory,
                    cancellation,
                )
                .await?
            }
            TtsRoute::Cloud(provider) => {
                crate::voice::cloud_tts::render_to_artifact(
                    &provider,
                    &text,
                    timeout_ms,
                    cancellation,
                    &cache_directory,
                )
                .await?
            }
            TtsRoute::Harness(_) => {
                return Err("Unresolved Harness TTS render route".to_string());
            }
        };
        finalize_rendered_chunk(sequence, boundary_at, path, started)
    })
}

fn finalize_rendered_chunk(
    sequence: u64,
    boundary_at: Instant,
    path: PathBuf,
    started: Instant,
) -> Result<RenderedChunk, String> {
    // Take ownership before validating the artifact so every error path removes it.
    let mut rendered = RenderedChunk {
        sequence,
        boundary_at,
        path,
        bytes: 0,
        audio_ms: 0,
        synthesis_ms: 0,
    };
    rendered.bytes = fs::metadata(&rendered.path)
        .map_err(|_| "Could not inspect rendered speech audio".to_string())?
        .len();
    rendered.audio_ms = wave_duration_ms(&rendered.path)?;
    rendered.synthesis_ms = started.elapsed().as_millis().try_into().unwrap_or(u64::MAX);
    Ok(rendered)
}

fn adaptive_render_concurrency(samples: &VecDeque<(u64, u64)>) -> usize {
    if samples.is_empty() {
        return 2;
    }
    let mut synthesis = samples.iter().map(|sample| sample.0).collect::<Vec<_>>();
    let mut audio = samples.iter().map(|sample| sample.1).collect::<Vec<_>>();
    synthesis.sort_unstable();
    audio.sort_unstable();
    let p95_index = ((synthesis.len() * 95).div_ceil(100)).saturating_sub(1);
    let p95 = synthesis[p95_index];
    let median = audio[audio.len() / 2].max(1);
    p95.div_ceil(median).clamp(1, MAX_RENDER_CONCURRENCY as u64) as usize
}

fn render_slots_used(rendering: usize, ready: usize, playing: bool) -> usize {
    rendering
        .saturating_add(ready)
        .saturating_add(usize::from(playing))
}

fn wave_duration_ms(path: &Path) -> Result<u64, String> {
    let mut file =
        fs::File::open(path).map_err(|_| "Could not inspect rendered speech audio".to_string())?;
    let mut header = [0_u8; 4_096];
    let length = file
        .read(&mut header)
        .map_err(|_| "Could not inspect rendered speech audio".to_string())?;
    if length < 44 || &header[..4] != b"RIFF" || &header[8..12] != b"WAVE" {
        return Err("Rendered speech audio was not a valid WAVE file".to_string());
    }
    let byte_rate = u32::from_le_bytes(header[28..32].try_into().expect("byte rate slice"));
    if byte_rate == 0 {
        return Err("Rendered speech audio had an invalid byte rate".to_string());
    }
    let mut cursor = 12_usize;
    while cursor + 8 <= length {
        let chunk_size = u32::from_le_bytes(
            header[cursor + 4..cursor + 8]
                .try_into()
                .expect("chunk size slice"),
        ) as usize;
        if &header[cursor..cursor + 4] == b"data" {
            return Ok((chunk_size as u64 * 1_000).div_ceil(byte_rate as u64));
        }
        cursor = cursor
            .saturating_add(8)
            .saturating_add(chunk_size + (chunk_size % 2));
    }
    Err("Rendered speech audio did not contain a bounded data chunk".to_string())
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

#[cfg(test)]
mod tests {
    use super::*;

    fn write_wave(path: &Path, byte_rate: u32, data_bytes: u32) {
        let mut wave = Vec::with_capacity(44 + data_bytes as usize);
        wave.extend_from_slice(b"RIFF");
        wave.extend_from_slice(&(36_u32 + data_bytes).to_le_bytes());
        wave.extend_from_slice(b"WAVEfmt ");
        wave.extend_from_slice(&16_u32.to_le_bytes());
        wave.extend_from_slice(&1_u16.to_le_bytes());
        wave.extend_from_slice(&1_u16.to_le_bytes());
        wave.extend_from_slice(&16_000_u32.to_le_bytes());
        wave.extend_from_slice(&byte_rate.to_le_bytes());
        wave.extend_from_slice(&2_u16.to_le_bytes());
        wave.extend_from_slice(&16_u16.to_le_bytes());
        wave.extend_from_slice(b"data");
        wave.extend_from_slice(&data_bytes.to_le_bytes());
        wave.resize(44 + data_bytes as usize, 0);
        fs::write(path, wave).expect("wave fixture writes");
    }

    #[test]
    fn disabling_a_session_cancels_and_removes_it() {
        let runtime = StreamingSpeechRuntime::default();
        let (work, _receiver) = mpsc::channel(1);
        let cancellation = Arc::new(RunCancellation::default());
        let (idle_reset, _idle_resets) = watch::channel(None);
        runtime.sessions.lock().expect("sessions lock").insert(
            "run-disable".to_string(),
            SpeechSession {
                accumulator: SentenceAccumulator::default(),
                work,
                cancellation: cancellation.clone(),
                child: Arc::new(Mutex::new(None)),
                closed: false,
                enabled: true,
                idle_timer: None,
                idle_reset,
            },
        );

        runtime.set_enabled("run-disable", false);

        assert!(cancellation.is_cancelled());
        assert!(!runtime.is_active());
    }

    #[test]
    fn adaptive_concurrency_is_bounded_and_uses_recent_latency_ratio() {
        assert_eq!(adaptive_render_concurrency(&VecDeque::new()), 2);
        assert_eq!(
            adaptive_render_concurrency(&VecDeque::from([(100, 1_000), (120, 1_000)])),
            1
        );
        assert_eq!(
            adaptive_render_concurrency(&VecDeque::from([
                (1_900, 1_000),
                (2_100, 1_000),
                (2_900, 1_000),
            ])),
            3
        );
        assert_eq!(
            adaptive_render_concurrency(&VecDeque::from([(99_000, 1)])),
            MAX_RENDER_CONCURRENCY
        );
    }

    #[test]
    fn player_and_render_ahead_share_the_three_chunk_bound() {
        assert_eq!(render_slots_used(0, 2, true), MAX_READY_CHUNKS);
        assert_eq!(render_slots_used(2, 1, false), MAX_READY_CHUNKS);
        assert!(render_slots_used(1, 0, true) < MAX_READY_CHUNKS);
    }

    #[test]
    fn wave_duration_uses_byte_rate_and_rejects_invalid_headers() {
        let path = std::env::temp_dir().join(format!(
            "saaa-tts-duration-{}.wav",
            uuid::Uuid::new_v4().simple()
        ));
        write_wave(&path, 32_000, 16_000);
        assert_eq!(wave_duration_ms(&path), Ok(500));
        fs::write(&path, b"not-wave").expect("invalid wave fixture writes");
        assert!(wave_duration_ms(&path).is_err());
        let _ = fs::remove_file(path);
    }

    #[test]
    fn invalid_rendered_artifact_is_removed_during_finalization() {
        let path = std::env::temp_dir().join(format!(
            "saaa-tts-invalid-artifact-{}.wav",
            uuid::Uuid::new_v4().simple()
        ));
        fs::write(&path, b"not-wave").expect("invalid wave fixture writes");

        assert!(finalize_rendered_chunk(0, Instant::now(), path.clone(), Instant::now()).is_err());
        assert!(!path.exists());
    }
}
