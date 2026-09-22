const MAX_QUEUED_CHUNKS: usize = 32;
const MAX_RENDER_CONCURRENCY: usize = 3;
const MAX_READY_CHUNKS: usize = 3;
const MAX_READY_AUDIO_BYTES: u64 = 16 * 1_024 * 1_024;
const MAX_READY_AUDIO_MS: u64 = 30_000;
/// Owns the sentence accumulator and renderer queue for every spoken turn so new
/// deltas can arrive while the previous sentence is playing.
#[derive(Clone, Default)]
pub(crate) struct StreamingSpeechRuntime {
    sessions: Arc<Mutex<HashMap<String, SpeechSession>>>,
    directives: Arc<Mutex<HashMap<String, crate::voice::cloud_tts::speech_directive::SpeechDirectiveProjection>>>,
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
    writer: Option<Arc<crate::persistence::SqliteWriter>>,
    expression: crate::voice::cloud_tts::speech_directive::SpeechExpression,
    expression_locked: bool,
}
enum SpeechWork {
    Chunk {
        text: String,
        expression: crate::voice::cloud_tts::speech_directive::SpeechExpression,
        boundary_at: Instant,
    },
    Finish,
}
fn queue_chunk(session: &mut SpeechSession, text: String) -> Result<(), String> {
    let boundary_at = Instant::now();
    let expression = session.expression;
    session.expression_locked = true;
    session
        .work
        .try_send(SpeechWork::Chunk {
            text,
            expression,
            boundary_at,
        })
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
    writer: Option<Arc<crate::persistence::SqliteWriter>>,
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
        voice_conversation: Option<&str>,
    ) -> Result<(), String> {
        let (mut route, _provider_id, timeout_ms) = selected_tts_route(state)?;
        if let Some(conversation) = voice_conversation.filter(|_| crate::larm_voice::enabled()) {
            let harness = state
                .sqlite_readers
                .read(|c| Ok(crate::persistence::load_model_providers(c)?.harness))?;
            fn apply(route: &mut TtsRoute, conversation: &str, harness: &crate::HarnessSettings) {
                match route {
                    TtsRoute::Harness(..) => {
                        *route = TtsRoute::Larm(conversation.to_string(), harness.clone())
                    }
                    TtsRoute::Fallback(routes, _) => {
                        for route in routes {
                            apply(route, conversation, harness);
                        }
                    }
                    _ => {}
                }
            }
            apply(&mut route, conversation, &harness);
        }
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
                writer: Some(state.sqlite_writer.clone()),
                expression: self.expression_for(run_id),
                expression_locked: false,
            },
        );
        drop(sessions);

        let sessions = self.sessions.clone();
        let run_id = run_id.to_string();
        let cache_directory = state.data_directory.join("tts-cache");
        let situation = state.situation.clone();
        let writer = state.sqlite_writer.clone();
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
                    writer: Some(writer),
                },
            )
            .await;
            if let Ok(mut active) = sessions.lock() {
                active.remove(&run_id);
            }
            situation.set_audio_state(crate::situation::contracts::AudioState::Silent);
            crate::larm_voice::speech_priority::finished(&run_id);
            let _ = on_event.send(RuntimeEvent::SpeechEnded { run_id });
        });
        Ok(())
    }

    pub(crate) fn project_delta(&self, run_id: &str, delta: &str) -> Result<String, String> {
        let mut directives = self
            .directives
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let parser = directives.entry(run_id.to_string()).or_default();
        let output = parser.push(delta);
        let decided = output.decided;
        drop(directives);
        if let Some(expression) = decided {
            self.set_expression(run_id, expression)?;
        }
        Ok(output.visible)
    }

    pub(crate) fn expression_for(
        &self,
        run_id: &str,
    ) -> crate::voice::cloud_tts::speech_directive::SpeechExpression {
        self.directives
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(run_id)
            .and_then(|parser| parser.expression())
            .unwrap_or_default()
    }

    pub(crate) fn set_expression(
        &self,
        run_id: &str,
        expression: crate::voice::cloud_tts::speech_directive::SpeechExpression,
    ) -> Result<(), String> {
        let mut sessions = match self.sessions.lock() {
            Ok(sessions) => sessions,
            Err(_) => return Ok(()),
        };
        let Some(session) = sessions.get_mut(run_id) else {
            return Ok(());
        };
        if session.expression_locked && session.expression != expression {
            return Err("Speech expression changed after synthesis started".into());
        }
        session.expression = expression;
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

    pub(crate) fn queue_utterance(&self, run_id: &str, text: &str) -> Result<(), String> {
        if text.chars().count() > MAX_SOURCE_CHARS {
            return Err("Speech utterance is too large".to_string());
        }
        let spoken = crate::voice_text::text_for_speech(text);
        if spoken.is_empty() {
            return Ok(());
        }
        let mut sessions = self
            .sessions
            .lock()
            .map_err(|_| "Streaming speech runtime lock unavailable".to_string())?;
        let Some(session) = sessions.get_mut(run_id) else {
            return Ok(());
        };
        if session.closed || session.cancellation.is_cancelled() || !session.enabled {
            return Ok(());
        }
        queue_chunk(session, spoken)
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
        self.finish_inner(run_id, None, final_content)
    }

    pub(crate) fn finish_message(
        &self,
        run_id: &str,
        message_id: &str,
        final_content: &str,
    ) -> Result<(), String> {
        self.finish_inner(run_id, Some(message_id), final_content)
    }

    fn finish_inner(
        &self,
        run_id: &str,
        message_id: Option<&str>,
        final_content: &str,
    ) -> Result<(), String> {
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
        if let Some(message_id) = message_id {
            let writer = session
                .writer
                .as_ref()
                .ok_or_else(|| "Speech delivery persistence is unavailable".to_string())?;
            let queued = writer.write(|connection| {
                let now = crate::now_iso();
                connection
                    .execute(
                        "INSERT OR IGNORE INTO speech_deliveries(id,runtime_run_id,message_id,status,created_at,updated_at) VALUES(?1,?2,?3,'queued',?4,?4)",
                        rusqlite::params![crate::new_id("speech-delivery"), run_id, message_id, now],
                    )
                    .map_err(crate::database_error)
            })?;
            if queued == 0 {
                return Ok(());
            }
        }
        if let Some(timer) = session.idle_timer.take() {
            let _ = session.idle_reset.send(None);
            timer.abort();
        }
        if let Err(error) = session.accumulator.finish(final_content) {
            if message_id.is_some() {
                if let Some(writer) = &session.writer {
                    let _ = writer.write(|connection| {
                        connection
                            .execute(
                                "UPDATE speech_deliveries SET status='failed',updated_at=?1 WHERE runtime_run_id=?2 AND message_id=?3 AND status='queued'",
                                rusqlite::params![crate::now_iso(), run_id, message_id],
                            )
                            .map_err(crate::database_error)?;
                        Ok(())
                    });
                }
            }
            return Err(format!("Invalid final streamed speech input: {error:?}"));
        }
        let mut final_chunks = Vec::new();
        while let Some(chunk) = session.accumulator.next_chunk(SelectReason::Completion) {
            final_chunks.push((chunk.spoken, Instant::now()));
        }
        session.closed = true;
        session.expression_locked = true;
        let session_expression = session.expression;
        let work = session.work.clone();
        tauri::async_runtime::spawn(async move {
            for (text, boundary_at) in final_chunks {
                if work
                    .send(SpeechWork::Chunk {
                        text,
                        expression: session_expression,
                        boundary_at,
                    })
                    .await
                    .is_err()
                {
                    return;
                }
                crate::runtime::event_hub::performance::record_tts_boundary_to_dispatch(
                    boundary_at.elapsed(),
                );
            }
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
        self.cancel_one(&format!("{run_id}_ack"));
        self.cancel_one(run_id);
    }

    fn cancel_one(&self, run_id: &str) {
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
    let externally_cancelled = context.cancellation.is_cancelled();
    let status = match (&result, externally_cancelled) {
        (_, true) => "cancelled",
        (Err(_), false) => "failed",
        (Ok(_), false) => "completed",
    };
    if let Some(writer) = &context.writer {
        let _ = writer.write(|connection| {
            connection
                .execute(
                    "UPDATE speech_deliveries SET status=?1,updated_at=?2 WHERE runtime_run_id=?3 AND status='queued'",
                    rusqlite::params![status, crate::now_iso(), context.run_id],
                )
                .map_err(crate::database_error)?;
            Ok(())
        });
    }
    if let Err(error) = result {
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
