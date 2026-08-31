use std::{
    collections::VecDeque,
    sync::{Arc, Weak},
    time::Duration,
};

use tokio::sync::{mpsc, oneshot, Semaphore};
use zeroize::{Zeroize, Zeroizing};

#[cfg(test)]
use tauri::ipc::Channel;

use super::{
    batch_engine::{BatchEngine, DecodeKind, DecodeRequest},
    batch_runtime::{BatchDecode, BatchDecodeOutcome},
    contracts::{CommitReason, VoiceAsrFailureCode, VoiceAsrStreamEvent},
    native_connection::{NativeConnection, NativeInbound},
    speaker_gate_runtime::SpeakerGate,
};
use crate::{persistence::audit::VoiceAsrAuditChannel, RunCancellation};

const FINAL_TIMEOUT: Duration = Duration::from_secs(15);
const RESULT_CAPACITY: usize = 8;

pub(crate) enum SessionCommand {
    Audio(Zeroizing<Vec<u8>>),
    Commit {
        reason: CommitReason,
        accepted: oneshot::Sender<Result<(), String>>,
    },
    Stop {
        finalize_current: bool,
        accepted: oneshot::Sender<Result<(), String>>,
    },
}

pub(crate) struct SessionConfig {
    pub(crate) session_id: String,
    pub(crate) current_utterance_id: String,
    pub(crate) event: VoiceAsrAuditChannel,
    pub(crate) batch_decoder: Arc<dyn BatchDecode>,
    pub(crate) native: Option<NativeConnection>,
    pub(crate) speaker_gate: SpeakerGate,
    pub(crate) allowed_languages: Vec<String>,
    pub(crate) final_timeout: Duration,
    pub(crate) cancellation: Arc<RunCancellation>,
    pub(crate) start_degraded: Option<VoiceAsrFailureCode>,
}

struct Utterance {
    id: String,
    pcm: Zeroizing<Vec<u8>>,
    engine: BatchEngine,
    revision: u64,
    provider_revision: u64,
    provider_stable: String,
    committed_at: Option<tokio::time::Instant>,
    final_scheduled: bool,
    partial_cancellation: Arc<RunCancellation>,
}

impl Utterance {
    fn new(id: String) -> Self {
        Self {
            id,
            pcm: Zeroizing::new(Vec::new()),
            engine: BatchEngine::default(),
            revision: 0,
            provider_revision: 0,
            provider_stable: String::new(),
            committed_at: None,
            final_scheduled: false,
            partial_cancellation: Arc::new(RunCancellation::default()),
        }
    }

    fn sample_count(&self) -> u64 {
        (self.pcm.len() / 2) as u64
    }

    fn has_speech(&self) -> bool {
        self.pcm.iter().any(|byte| *byte != 0)
    }
}

struct DecodeResult {
    utterance_id: String,
    request: DecodeRequest,
    result: Result<BatchDecodeOutcome, String>,
}

struct Actor {
    session_id: String,
    event: VoiceAsrAuditChannel,
    commands: mpsc::Receiver<SessionCommand>,
    decoder: Arc<dyn BatchDecode>,
    decode_tx: mpsc::Sender<DecodeResult>,
    decode_rx: mpsc::Receiver<DecodeResult>,
    decode_slot: Arc<Semaphore>,
    decode_cancellations: Vec<Weak<RunCancellation>>,
    native: Option<NativeConnection>,
    gate: SpeakerGate,
    allowed_languages: Vec<String>,
    final_timeout: Duration,
    cancellation: Arc<RunCancellation>,
    current: Utterance,
    pending: VecDeque<Utterance>,
    batch_failures: usize,
    degraded: bool,
    stopping: bool,
    stop_deadline: Option<tokio::time::Instant>,
}

pub(crate) async fn run(config: SessionConfig, commands: mpsc::Receiver<SessionCommand>) {
    let (decode_tx, decode_rx) = mpsc::channel(RESULT_CAPACITY);
    let current = Utterance::new(config.current_utterance_id);
    let mut actor = Actor {
        session_id: config.session_id,
        event: config.event,
        commands,
        decoder: config.batch_decoder,
        decode_tx,
        decode_rx,
        decode_slot: Arc::new(Semaphore::new(1)),
        decode_cancellations: Vec::new(),
        native: config.native,
        gate: config.speaker_gate,
        allowed_languages: config.allowed_languages,
        final_timeout: config.final_timeout,
        cancellation: config.cancellation,
        current,
        pending: VecDeque::new(),
        batch_failures: 0,
        degraded: false,
        stopping: false,
        stop_deadline: None,
    };
    if let Some(reason) = config.start_degraded {
        actor.emit_degraded(reason);
    }
    actor.run_loop().await;
}

impl Actor {
    async fn run_loop(&mut self) {
        let mut deadline_tick = tokio::time::interval(Duration::from_millis(250));
        deadline_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            if self.stopping && self.pending.is_empty() {
                self.finish_stop().await;
                return;
            }
            tokio::select! {
                biased;
                _ = self.cancellation.cancelled() => {
                    self.discard_everything("cancelled");
                    self.finish_stop().await;
                    return;
                }
                command = self.commands.recv() => {
                    if self.handle_command(command).await {
                        return;
                    }
                }
                Some(result) = self.decode_rx.recv() => {
                    if self.handle_decode_result(result).await {
                        self.cancel_decodes();
                        self.finish_stop().await;
                        return;
                    }
                }
                inbound = next_native(&mut self.native) => {
                    self.handle_native(inbound).await;
                }
                _ = deadline_tick.tick() => {
                    if self.check_deadlines().await {
                        return;
                    }
                }
            }
        }
    }

    async fn handle_command(&mut self, command: Option<SessionCommand>) -> bool {
        match command {
            Some(SessionCommand::Audio(packet)) if !self.stopping => {
                let sanitized = self.gate.push(packet).await;
                self.process_sanitized(sanitized).await;
                false
            }
            Some(SessionCommand::Audio(mut packet)) => {
                packet.fill(0);
                false
            }
            Some(SessionCommand::Commit { reason, accepted }) if !self.stopping => {
                let _reason = reason;
                if self.pending.len() >= 2 {
                    let _ = accepted.send(Err("asr-backpressure".to_string()));
                    return false;
                }
                let sanitized = self.gate.flush().await;
                self.process_sanitized(sanitized).await;
                let result = self.commit_current().await;
                let _ = accepted.send(result);
                false
            }
            Some(SessionCommand::Commit { accepted, .. }) => {
                let _ = accepted.send(Err("asr-session-not-found".to_string()));
                false
            }
            Some(SessionCommand::Stop {
                finalize_current,
                accepted,
            }) => {
                if !finalize_current {
                    let _ = accepted.send(Ok(()));
                    self.cancellation.cancel();
                    self.discard_everything("cancelled");
                    if let Some(mut native) = self.native.take() {
                        let _ = native.sink.stop(&self.session_id).await;
                    }
                    self.emit_stopped();
                    return true;
                }
                self.stopping = true;
                self.stop_deadline = Some(tokio::time::Instant::now() + FINAL_TIMEOUT);
                let _ = accepted.send(Ok(()));
                let sanitized = self.gate.flush().await;
                self.process_sanitized(sanitized).await;
                if !self.current.pcm.is_empty() {
                    if self.pending.len() >= 2 {
                        self.emit_failed(
                            Some(self.current.id.clone()),
                            VoiceAsrFailureCode::Backpressure,
                            "Too many final utterances are pending",
                            false,
                        );
                        self.emit_discarded(&self.current.id.clone(), "cancelled");
                    } else if let Err(error) = self.commit_current().await {
                        self.emit_failed(
                            Some(self.current.id.clone()),
                            failure_code(&error),
                            &error,
                            false,
                        );
                    }
                }
                false
            }
            None => {
                self.cancellation.cancel();
                self.discard_everything("cancelled");
                self.emit_stopped();
                true
            }
        }
    }

    async fn process_sanitized(&mut self, packets: Vec<Zeroizing<Vec<u8>>>) {
        for packet in packets {
            self.current.pcm.extend_from_slice(&packet);
            if let Some(native) = &mut self.native {
                if native.sink.audio(packet).await.is_err() {
                    self.fallback_to_batch(VoiceAsrFailureCode::StreamTimeout)
                        .await;
                }
            }
            if self.native.is_none() {
                if let Some(request) = self.current.engine.on_audio(self.current.sample_count()) {
                    self.spawn_decode(self.current.id.clone(), request, self.current.pcm.clone());
                }
            }
        }
    }

    async fn commit_current(&mut self) -> Result<(), String> {
        let next_id = crate::new_id("voice_asr_utterance");
        let end_sample = self.current.sample_count();
        self.current.partial_cancellation.cancel();
        self.current.committed_at = Some(tokio::time::Instant::now());
        let mut committed = std::mem::replace(&mut self.current, Utterance::new(next_id.clone()));
        if let Some(native) = &mut self.native {
            let result = native
                .sink
                .commit(&committed.id, &next_id, end_sample)
                .await;
            self.pending.push_back(committed);
            if let Err(error) = result {
                self.fallback_to_batch(failure_code(&error)).await;
            }
            return Ok(());
        }
        if end_sample == 0 {
            self.emit_discarded(&committed.id, "no-speech");
            return Ok(());
        }
        let request = committed.engine.commit(end_sample);
        committed.final_scheduled = true;
        self.spawn_decode(committed.id.clone(), request, committed.pcm.clone());
        self.pending.push_back(committed);
        Ok(())
    }

    fn spawn_decode(
        &mut self,
        utterance_id: String,
        request: DecodeRequest,
        mut pcm: Zeroizing<Vec<u8>>,
    ) {
        let byte_end = (request.end_sample as usize)
            .saturating_mul(2)
            .min(pcm.len());
        pcm.truncate(byte_end);
        let decoder = self.decoder.clone();
        let cancellation = match request.kind {
            DecodeKind::Partial => self.current.partial_cancellation.clone(),
            DecodeKind::Final => Arc::new(RunCancellation::default()),
        };
        track_decode_cancellation(&mut self.decode_cancellations, &cancellation);
        let decode_slot = self.decode_slot.clone();
        let result_tx = self.decode_tx.clone();
        tokio::spawn(async move {
            let result = tokio::select! {
                _ = cancellation.cancelled() => Err("Transcription cancelled".to_string()),
                permit = decode_slot.acquire_owned() => match permit {
                    Ok(_permit) => decoder.decode(pcm, cancellation).await,
                    Err(_) => Err("ASR decoder unavailable".to_string()),
                },
            };
            let _ = result_tx
                .send(DecodeResult {
                    utterance_id,
                    request,
                    result,
                })
                .await;
        });
    }

    async fn handle_decode_result(&mut self, decoded: DecodeResult) -> bool {
        match decoded.request.kind {
            DecodeKind::Partial => self.handle_partial_decode(decoded),
            DecodeKind::Final => self.handle_final_decode(decoded),
        }
    }

    fn handle_partial_decode(&mut self, decoded: DecodeResult) -> bool {
        if decoded.utterance_id != self.current.id || self.native.is_some() {
            return false;
        }
        match decoded.result {
            Ok(BatchDecodeOutcome::Transcript { text, language }) => {
                self.batch_failures = 0;
                let (projection, next) = self
                    .current
                    .engine
                    .on_partial_complete(decoded.request.end_sample, &text);
                if let Some(projection) = projection {
                    self.current.revision += 1;
                    self.send_event(VoiceAsrStreamEvent::Partial {
                        session_id: self.session_id.clone(),
                        utterance_id: self.current.id.clone(),
                        revision: self.current.revision,
                        start_ms: 0,
                        end_ms: samples_to_ms(decoded.request.end_sample),
                        stable_text: projection.stable,
                        unstable_text: projection.unstable,
                        language,
                    });
                }
                if let Some(next) = next {
                    self.spawn_decode(self.current.id.clone(), next, self.current.pcm.clone());
                }
                false
            }
            Ok(BatchDecodeOutcome::NoSpeech) => {
                self.batch_failures = 0;
                if let Some(next) = self
                    .current
                    .engine
                    .on_partial_failed(decoded.request.end_sample)
                {
                    self.spawn_decode(self.current.id.clone(), next, self.current.pcm.clone());
                }
                false
            }
            Err(error) => {
                if let Some(next) = self
                    .current
                    .engine
                    .on_partial_failed(decoded.request.end_sample)
                {
                    self.spawn_decode(self.current.id.clone(), next, self.current.pcm.clone());
                }
                self.record_batch_failure(Some(decoded.utterance_id), &error)
            }
        }
    }

    fn handle_final_decode(&mut self, decoded: DecodeResult) -> bool {
        let Some(index) = self
            .pending
            .iter()
            .position(|utterance| utterance.id == decoded.utterance_id)
        else {
            return false;
        };
        let mut utterance = self.pending.remove(index).expect("index was checked");
        match decoded.result {
            Ok(BatchDecodeOutcome::Transcript { text, language }) => {
                self.batch_failures = 0;
                utterance.revision += 1;
                self.emit_final(
                    &utterance,
                    utterance.revision,
                    0,
                    utterance.sample_count(),
                    text,
                    language,
                );
                false
            }
            Ok(BatchDecodeOutcome::NoSpeech) => {
                self.batch_failures = 0;
                let reason = if utterance.has_speech() {
                    "no-speech"
                } else if self.gate.scope() == "target-speaker" {
                    "target-speaker-empty"
                } else {
                    "no-speech"
                };
                self.emit_discarded(&utterance.id, reason);
                false
            }
            Err(error) => self.record_batch_failure(Some(utterance.id), &error),
        }
    }

    async fn handle_native(&mut self, inbound: NativeInbound) {
        match inbound {
            NativeInbound::Ping(payload) => {
                if let Some(native) = &mut self.native {
                    if native.sink.pong(payload).await.is_err() {
                        self.fallback_to_batch(VoiceAsrFailureCode::StreamTimeout)
                            .await;
                    }
                }
            }
            NativeInbound::Closed(reason) => self.fallback_to_batch(failure_code(reason)).await,
            NativeInbound::Provider(event) => {
                if let Err(error) = self.apply_provider_event(event) {
                    self.fallback_to_batch(failure_code(&error)).await;
                }
            }
        }
    }

    fn apply_provider_event(
        &mut self,
        event: super::harness_stream::ProviderEvent,
    ) -> Result<(), String> {
        use super::harness_stream::ProviderEvent;
        match event {
            ProviderEvent::Ready { .. } => Err("asr-stream-protocol".to_string()),
            ProviderEvent::Partial {
                session_id,
                utterance_id,
                revision,
                start_sample,
                end_sample,
                stable_text,
                unstable_text,
                language,
            } => {
                self.validate_session(&session_id)?;
                if language.is_some() {
                    self.validate_language(language.as_deref())?;
                }
                let current = &mut self.current;
                if current.id != utterance_id || revision <= current.provider_revision {
                    return Ok(());
                }
                validate_provider_update(current, revision, start_sample, end_sample)?;
                if !stable_text.starts_with(&current.provider_stable) {
                    return Err("asr-stream-protocol".to_string());
                }
                current.provider_revision = revision;
                current.provider_stable = stable_text.clone();
                current.revision += 1;
                let emitted_revision = current.revision;
                self.send_event(VoiceAsrStreamEvent::Partial {
                    session_id: self.session_id.clone(),
                    utterance_id,
                    revision: emitted_revision,
                    start_ms: samples_to_ms(start_sample),
                    end_ms: samples_to_ms(end_sample),
                    stable_text,
                    unstable_text,
                    language,
                });
                Ok(())
            }
            ProviderEvent::Final {
                session_id,
                utterance_id,
                revision,
                start_sample,
                end_sample,
                text,
                language,
            } => {
                self.validate_session(&session_id)?;
                self.validate_language(language.as_deref())?;
                if text.trim().is_empty() {
                    return Err("asr-stream-protocol".to_string());
                }
                let Some(index) = self.pending.iter().position(|item| item.id == utterance_id)
                else {
                    return Ok(());
                };
                let pending = &self.pending[index];
                if revision <= pending.provider_revision {
                    return Ok(());
                }
                validate_provider_update(pending, revision, start_sample, end_sample)?;
                let mut utterance = self.pending.remove(index).expect("index was checked");
                utterance.provider_revision = revision;
                utterance.revision += 1;
                self.emit_final(
                    &utterance,
                    utterance.revision,
                    start_sample,
                    end_sample,
                    text,
                    language,
                );
                Ok(())
            }
            ProviderEvent::NoSpeech {
                session_id,
                utterance_id,
                revision,
                start_sample,
                end_sample,
            } => {
                self.validate_session(&session_id)?;
                let Some(index) = self.pending.iter().position(|item| item.id == utterance_id)
                else {
                    return Ok(());
                };
                let pending = &self.pending[index];
                if revision <= pending.provider_revision {
                    return Ok(());
                }
                validate_provider_update(pending, revision, start_sample, end_sample)?;
                let utterance = self.pending.remove(index).expect("index was checked");
                let reason = if self.gate.scope() == "target-speaker" && !utterance.has_speech() {
                    "target-speaker-empty"
                } else {
                    "no-speech"
                };
                self.emit_discarded(&utterance_id, reason);
                Ok(())
            }
            ProviderEvent::Error {
                session_id,
                utterance_id,
                code: mut provider_code,
                message: mut provider_message,
                recoverable,
            } => {
                provider_code.zeroize();
                provider_message.zeroize();
                self.validate_session(&session_id)?;
                self.emit_failed(
                    utterance_id.clone(),
                    if recoverable {
                        VoiceAsrFailureCode::StreamTimeout
                    } else {
                        VoiceAsrFailureCode::StreamProtocol
                    },
                    if recoverable {
                        "Native ASR requested batch fallback"
                    } else {
                        "Native ASR protocol failed"
                    },
                    false,
                );
                if recoverable {
                    Err("asr-stream-timeout".to_string())
                } else {
                    Err("asr-stream-protocol".to_string())
                }
            }
            ProviderEvent::Stopped { session_id } => {
                self.validate_session(&session_id)?;
                Err("asr-stream-timeout".to_string())
            }
        }
    }

    async fn fallback_to_batch(&mut self, reason: VoiceAsrFailureCode) {
        if self.native.take().is_none() {
            return;
        }
        self.emit_degraded(reason);
        let mut requests = Vec::new();
        for utterance in &mut self.pending {
            if !utterance.final_scheduled {
                utterance.partial_cancellation.cancel();
                let request = utterance.engine.commit(utterance.sample_count());
                utterance.final_scheduled = true;
                requests.push((utterance.id.clone(), request, utterance.pcm.clone()));
            }
        }
        for (id, request, pcm) in requests {
            self.spawn_decode(id, request, pcm);
        }
        if let Some(request) = self.current.engine.on_audio(self.current.sample_count()) {
            self.spawn_decode(self.current.id.clone(), request, self.current.pcm.clone());
        }
    }

    async fn check_deadlines(&mut self) -> bool {
        let now = tokio::time::Instant::now();
        if self.native.is_some()
            && self.pending.iter().any(|utterance| {
                utterance
                    .committed_at
                    .is_some_and(|started| now.duration_since(started) >= self.final_timeout)
            })
        {
            self.fallback_to_batch(VoiceAsrFailureCode::FinalTimeout)
                .await;
        }
        if self.stop_deadline.is_some_and(|deadline| now >= deadline) {
            for utterance in self.pending.drain(..).collect::<Vec<_>>() {
                self.emit_failed(
                    Some(utterance.id),
                    VoiceAsrFailureCode::FinalTimeout,
                    "ASR finalization timed out",
                    false,
                );
            }
            self.finish_stop().await;
            return true;
        }
        false
    }

    fn record_batch_failure(&mut self, utterance_id: Option<String>, error: &str) -> bool {
        let (count, fatal) = next_failure(self.batch_failures);
        self.batch_failures = count;
        let code = failure_code(error);
        self.emit_failed(utterance_id, code, failure_message(code), fatal);
        fatal
    }

    fn validate_session(&self, session_id: &str) -> Result<(), String> {
        (session_id == self.session_id)
            .then_some(())
            .ok_or_else(|| "asr-stream-protocol".to_string())
    }

    fn validate_language(&self, language: Option<&str>) -> Result<(), String> {
        crate::voice::language::enforce_allowed_language(language, &self.allowed_languages)
            .map_err(|_| "asr-language-not-allowed".to_string())
    }

    fn emit_final(
        &self,
        utterance: &Utterance,
        revision: u64,
        start_sample: u64,
        end_sample: u64,
        text: String,
        language: Option<String>,
    ) {
        self.send_event(VoiceAsrStreamEvent::Final {
            session_id: self.session_id.clone(),
            utterance_id: utterance.id.clone(),
            revision,
            start_ms: samples_to_ms(start_sample),
            end_ms: samples_to_ms(end_sample),
            text,
            language,
        });
    }

    fn emit_discarded(&self, utterance_id: &str, reason: &'static str) {
        self.send_event(VoiceAsrStreamEvent::UtteranceDiscarded {
            session_id: self.session_id.clone(),
            utterance_id: utterance_id.to_string(),
            reason,
        });
    }

    fn emit_failed(
        &self,
        utterance_id: Option<String>,
        code: VoiceAsrFailureCode,
        message: &str,
        fatal: bool,
    ) {
        self.send_event(VoiceAsrStreamEvent::Failed {
            session_id: self.session_id.clone(),
            utterance_id,
            code,
            message: crate::redact_runtime_text(message),
            recovery: "Check the ASR provider connection and microphone, then retry.".to_string(),
            fatal,
        });
    }

    fn emit_degraded(&mut self, reason: VoiceAsrFailureCode) {
        if self.degraded {
            return;
        }
        self.degraded = true;
        self.send_event(VoiceAsrStreamEvent::Degraded {
            session_id: self.session_id.clone(),
            from: "native",
            to: "batch-agreement",
            reason_code: reason,
        });
    }

    fn discard_everything(&mut self, reason: &'static str) {
        self.cancel_decodes();
        if !self.current.pcm.is_empty() {
            self.emit_discarded(&self.current.id.clone(), reason);
        }
        for utterance in self.pending.drain(..).collect::<Vec<_>>() {
            self.emit_discarded(&utterance.id, reason);
        }
    }

    fn cancel_decodes(&mut self) {
        for cancellation in self
            .decode_cancellations
            .drain(..)
            .filter_map(|value| value.upgrade())
        {
            cancellation.cancel();
        }
    }

    async fn finish_stop(&mut self) {
        if let Some(mut native) = self.native.take() {
            let _ = native.sink.stop(&self.session_id).await;
        }
        self.emit_stopped();
    }

    fn emit_stopped(&self) {
        self.send_event(VoiceAsrStreamEvent::Stopped {
            session_id: self.session_id.clone(),
        });
    }

    fn send_event(&self, event: VoiceAsrStreamEvent) {
        if self.event.send(event).is_err() {
            self.cancellation.cancel();
        }
    }
}

fn track_decode_cancellation(
    tracked: &mut Vec<Weak<RunCancellation>>,
    cancellation: &Arc<RunCancellation>,
) {
    tracked.retain(|value| value.strong_count() > 0);
    let cancellation = Arc::downgrade(cancellation);
    if !tracked.iter().any(|value| value.ptr_eq(&cancellation)) {
        tracked.push(cancellation);
    }
}

async fn next_native(native: &mut Option<NativeConnection>) -> NativeInbound {
    match native {
        Some(connection) => connection
            .events
            .recv()
            .await
            .unwrap_or(NativeInbound::Closed("asr-stream-timeout")),
        None => std::future::pending().await,
    }
}

fn validate_provider_update(
    utterance: &Utterance,
    revision: u64,
    start_sample: u64,
    end_sample: u64,
) -> Result<(), String> {
    if revision == 0 || start_sample > end_sample || end_sample > utterance.sample_count() {
        return Err("asr-stream-protocol".to_string());
    }
    Ok(())
}

fn samples_to_ms(samples: u64) -> u64 {
    samples.saturating_mul(1_000) / 16_000
}

fn next_failure(current: usize) -> (usize, bool) {
    let next = current.saturating_add(1);
    (next, next >= 3)
}

fn failure_code(error: &str) -> VoiceAsrFailureCode {
    if error.contains("asr-stream-protocol") {
        VoiceAsrFailureCode::StreamProtocol
    } else if error.contains("asr-stream-timeout") {
        VoiceAsrFailureCode::StreamTimeout
    } else if error.contains("asr-final-timeout")
        || error.to_ascii_lowercase().contains("timed out")
        || error.to_ascii_lowercase().contains("timeout")
    {
        VoiceAsrFailureCode::FinalTimeout
    } else if error.contains("asr-language-not-allowed")
        || error.contains("ASR_LANGUAGE_NOT_ALLOWED")
        || error.contains("ASR_LANGUAGE_UNKNOWN")
    {
        VoiceAsrFailureCode::LanguageNotAllowed
    } else if error.contains("asr-target-speaker-unavailable") {
        VoiceAsrFailureCode::TargetSpeakerUnavailable
    } else if error.contains("asr-backpressure") {
        VoiceAsrFailureCode::Backpressure
    } else if error.contains("asr-cancelled") || error.to_ascii_lowercase().contains("cancelled") {
        VoiceAsrFailureCode::Cancelled
    } else {
        VoiceAsrFailureCode::ProviderUnavailable
    }
}

fn failure_message(code: VoiceAsrFailureCode) -> &'static str {
    match code {
        VoiceAsrFailureCode::StreamProtocol => "ASR streaming protocol failed",
        VoiceAsrFailureCode::StreamTimeout => "ASR streaming timed out",
        VoiceAsrFailureCode::FinalTimeout => "ASR finalization timed out",
        VoiceAsrFailureCode::LanguageNotAllowed => "ASR returned a disallowed language",
        VoiceAsrFailureCode::TargetSpeakerUnavailable => {
            "Target-speaker verification is unavailable"
        }
        VoiceAsrFailureCode::Backpressure => "ASR input exceeded its bounded capacity",
        VoiceAsrFailureCode::Cancelled => "ASR was cancelled",
        _ => "ASR provider failed",
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use async_trait::async_trait;
    use serde_json::Value;

    use super::*;
    use crate::voice::streaming_asr::native_connection::{fake_connection, NativeAsrSink};
    use crate::voice::streaming_asr::speaker_gate_runtime::SpeakerScorer;

    struct FixedDecoder;
    #[async_trait]
    impl BatchDecode for FixedDecoder {
        async fn decode(
            &self,
            _pcm16le: Zeroizing<Vec<u8>>,
            _cancellation: Arc<RunCancellation>,
        ) -> Result<BatchDecodeOutcome, String> {
            Ok(BatchDecodeOutcome::Transcript {
                text: "hello".to_string(),
                language: Some("en".to_string()),
            })
        }
    }

    struct BlockingDecoder(Arc<tokio::sync::Notify>);
    #[async_trait]
    impl BatchDecode for BlockingDecoder {
        async fn decode(
            &self,
            _pcm16le: Zeroizing<Vec<u8>>,
            _cancellation: Arc<RunCancellation>,
        ) -> Result<BatchDecodeOutcome, String> {
            self.0.notified().await;
            Ok(BatchDecodeOutcome::Transcript {
                text: "hello".to_string(),
                language: Some("en".to_string()),
            })
        }
    }

    struct NoopSink;
    #[async_trait]
    impl NativeAsrSink for NoopSink {
        async fn audio(&mut self, _bytes: Zeroizing<Vec<u8>>) -> Result<(), String> {
            Ok(())
        }
        async fn commit(
            &mut self,
            _utterance_id: &str,
            _next_utterance_id: &str,
            _end_sample: u64,
        ) -> Result<(), String> {
            Ok(())
        }
        async fn stop(&mut self, _session_id: &str) -> Result<(), String> {
            Ok(())
        }
        async fn pong(&mut self, _payload: Vec<u8>) -> Result<(), String> {
            Ok(())
        }
    }

    struct RecordingSink(Arc<Mutex<Vec<String>>>);
    #[async_trait]
    impl NativeAsrSink for RecordingSink {
        async fn audio(&mut self, bytes: Zeroizing<Vec<u8>>) -> Result<(), String> {
            self.0
                .lock()
                .unwrap()
                .push(format!("audio:{}", bytes.len()));
            Ok(())
        }
        async fn commit(
            &mut self,
            utterance_id: &str,
            next_utterance_id: &str,
            end_sample: u64,
        ) -> Result<(), String> {
            self.0.lock().unwrap().push(format!(
                "commit:{utterance_id}:{next_utterance_id}:{end_sample}"
            ));
            Ok(())
        }
        async fn stop(&mut self, session_id: &str) -> Result<(), String> {
            self.0.lock().unwrap().push(format!("stop:{session_id}"));
            Ok(())
        }
        async fn pong(&mut self, _payload: Vec<u8>) -> Result<(), String> {
            Ok(())
        }
    }

    struct RecordingDecoder {
        seen: Arc<Mutex<Vec<Vec<u8>>>>,
    }
    #[async_trait]
    impl BatchDecode for RecordingDecoder {
        async fn decode(
            &self,
            pcm16le: Zeroizing<Vec<u8>>,
            _cancellation: Arc<RunCancellation>,
        ) -> Result<BatchDecodeOutcome, String> {
            self.seen.lock().unwrap().push(pcm16le.to_vec());
            Ok(BatchDecodeOutcome::NoSpeech)
        }
    }

    struct RejectScorer;
    impl SpeakerScorer for RejectScorer {
        fn score(&self, _samples_16k: Zeroizing<Vec<f32>>) -> Result<f32, String> {
            Ok(0.1)
        }
        fn threshold(&self) -> f32 {
            0.5
        }
    }

    fn event_channel() -> (Channel<VoiceAsrStreamEvent>, Arc<Mutex<Vec<Value>>>) {
        let values = Arc::new(Mutex::new(Vec::new()));
        let captured = values.clone();
        let channel = Channel::new(move |body| {
            if let tauri::ipc::InvokeResponseBody::Json(json) = body {
                captured
                    .lock()
                    .expect("capture lock")
                    .push(serde_json::from_str(&json).expect("event json"));
            }
            Ok(())
        });
        (channel, values)
    }

    fn failing_event_channel() -> Channel<VoiceAsrStreamEvent> {
        Channel::new(|_| {
            Err(std::io::Error::new(std::io::ErrorKind::BrokenPipe, "channel closed").into())
        })
    }

    fn config(
        event: Channel<VoiceAsrStreamEvent>,
        native: Option<NativeConnection>,
    ) -> SessionConfig {
        SessionConfig {
            session_id: "session_test".to_string(),
            current_utterance_id: "utterance_test".to_string(),
            event: VoiceAsrAuditChannel::plain(event),
            batch_decoder: Arc::new(FixedDecoder),
            native,
            speaker_gate: SpeakerGate::new(None, 0.008),
            allowed_languages: vec!["en".to_string()],
            final_timeout: FINAL_TIMEOUT,
            cancellation: Arc::new(RunCancellation::default()),
            start_degraded: None,
        }
    }

    #[tokio::test]
    async fn batch_commit_emits_final_before_stopped() {
        let (event, captured) = event_channel();
        let (command_tx, command_rx) = mpsc::channel(8);
        let task = tokio::spawn(run(config(event, None), command_rx));
        command_tx
            .send(SessionCommand::Audio(Zeroizing::new(vec![1; 3_200])))
            .await
            .unwrap();
        let (accepted_tx, accepted_rx) = oneshot::channel();
        command_tx
            .send(SessionCommand::Commit {
                reason: CommitReason::Silence,
                accepted: accepted_tx,
            })
            .await
            .unwrap();
        accepted_rx.await.unwrap().unwrap();
        let (completed_tx, completed_rx) = oneshot::channel();
        command_tx
            .send(SessionCommand::Stop {
                finalize_current: true,
                accepted: completed_tx,
            })
            .await
            .unwrap();
        completed_rx.await.unwrap().unwrap();
        task.await.unwrap();
        let types = captured
            .lock()
            .unwrap()
            .iter()
            .map(|value| value["type"].as_str().unwrap().to_string())
            .collect::<Vec<_>>();
        assert_eq!(types, vec!["final", "stopped"]);
    }

    #[tokio::test]
    async fn stop_acknowledges_before_asynchronous_finalization_finishes() {
        let (event, captured) = event_channel();
        let release = Arc::new(tokio::sync::Notify::new());
        let mut config = config(event, None);
        config.batch_decoder = Arc::new(BlockingDecoder(release.clone()));
        let (command_tx, command_rx) = mpsc::channel(8);
        let task = tokio::spawn(run(config, command_rx));
        command_tx
            .send(SessionCommand::Audio(Zeroizing::new(vec![1; 3_200])))
            .await
            .unwrap();
        let (accepted_tx, accepted_rx) = oneshot::channel();
        command_tx
            .send(SessionCommand::Stop {
                finalize_current: true,
                accepted: accepted_tx,
            })
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_millis(100), accepted_rx)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert!(!task.is_finished());
        release.notify_one();
        task.await.unwrap();
        let types = captured
            .lock()
            .unwrap()
            .iter()
            .map(|value| value["type"].as_str().unwrap().to_string())
            .collect::<Vec<_>>();
        assert_eq!(types, vec!["final", "stopped"]);
    }

    #[tokio::test]
    async fn native_transport_failure_degrades_only_once() {
        let (event, captured) = event_channel();
        let (native, inbound) = fake_connection(Box::new(NoopSink));
        let (command_tx, command_rx) = mpsc::channel(8);
        let task = tokio::spawn(run(config(event, Some(native)), command_rx));
        inbound
            .send(NativeInbound::Closed("asr-stream-timeout"))
            .await
            .unwrap();
        tokio::task::yield_now().await;
        let (completed_tx, completed_rx) = oneshot::channel();
        command_tx
            .send(SessionCommand::Stop {
                finalize_current: false,
                accepted: completed_tx,
            })
            .await
            .unwrap();
        completed_rx.await.unwrap().unwrap();
        task.await.unwrap();
        let degraded = captured
            .lock()
            .unwrap()
            .iter()
            .filter(|value| value["type"] == "degraded")
            .count();
        assert_eq!(degraded, 1);
    }

    #[tokio::test]
    async fn recoverable_provider_error_falls_back_without_losing_the_final() {
        let (event, captured) = event_channel();
        let (native, inbound) = fake_connection(Box::new(NoopSink));
        let (command_tx, command_rx) = mpsc::channel(8);
        let task = tokio::spawn(run(config(event, Some(native)), command_rx));
        command_tx
            .send(SessionCommand::Audio(Zeroizing::new(vec![1; 3_200])))
            .await
            .unwrap();
        let (accepted_tx, accepted_rx) = oneshot::channel();
        command_tx
            .send(SessionCommand::Commit {
                reason: CommitReason::Silence,
                accepted: accepted_tx,
            })
            .await
            .unwrap();
        accepted_rx.await.unwrap().unwrap();
        inbound
            .send(NativeInbound::Provider(
                super::super::harness_stream::ProviderEvent::Error {
                    session_id: "session_test".to_string(),
                    utterance_id: Some("utterance_test".to_string()),
                    code: "temporary".to_string(),
                    message: "retry with batch".to_string(),
                    recoverable: true,
                },
            ))
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                if captured
                    .lock()
                    .unwrap()
                    .iter()
                    .any(|value| value["type"] == "final")
                {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        let (accepted_tx, accepted_rx) = oneshot::channel();
        command_tx
            .send(SessionCommand::Stop {
                finalize_current: false,
                accepted: accepted_tx,
            })
            .await
            .unwrap();
        accepted_rx.await.unwrap().unwrap();
        task.await.unwrap();
        let types = captured
            .lock()
            .unwrap()
            .iter()
            .map(|value| value["type"].as_str().unwrap().to_string())
            .collect::<Vec<_>>();
        assert!(types
            .windows(3)
            .any(|window| window == ["failed", "degraded", "final"]));
        let values = captured.lock().unwrap();
        let failed = values
            .iter()
            .find(|value| value["type"] == "failed")
            .unwrap();
        assert_eq!(failed["message"], "Native ASR requested batch fallback");
        assert!(!failed["message"]
            .as_str()
            .unwrap()
            .contains("retry with batch"));
    }

    #[tokio::test]
    async fn event_channel_failure_cancels_the_session() {
        let mut config = config(failing_event_channel(), None);
        config.start_degraded = Some(VoiceAsrFailureCode::StreamTimeout);
        let (_command_tx, command_rx) = mpsc::channel(1);
        tokio::time::timeout(Duration::from_secs(1), run(config, command_rx))
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn closed_native_event_channel_degrades_without_spinning() {
        let (event, captured) = event_channel();
        let (native, inbound) = fake_connection(Box::new(NoopSink));
        let (command_tx, command_rx) = mpsc::channel(8);
        let task = tokio::spawn(run(config(event, Some(native)), command_rx));
        drop(inbound);
        tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                if captured
                    .lock()
                    .unwrap()
                    .iter()
                    .any(|value| value["type"] == "degraded")
                {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        let (accepted_tx, accepted_rx) = oneshot::channel();
        command_tx
            .send(SessionCommand::Stop {
                finalize_current: false,
                accepted: accepted_tx,
            })
            .await
            .unwrap();
        accepted_rx.await.unwrap().unwrap();
        task.await.unwrap();
        let events = captured.lock().unwrap();
        assert_eq!(
            events
                .iter()
                .filter(|value| value["type"] == "degraded")
                .count(),
            1
        );
        assert_eq!(events.last().unwrap()["type"], "stopped");
    }

    #[tokio::test]
    async fn native_audio_commit_final_and_stop_preserve_wire_order() {
        let (event, captured) = event_channel();
        let writes = Arc::new(Mutex::new(Vec::new()));
        let (native, inbound) = fake_connection(Box::new(RecordingSink(writes.clone())));
        let (command_tx, command_rx) = mpsc::channel(8);
        let task = tokio::spawn(run(config(event, Some(native)), command_rx));
        command_tx
            .send(SessionCommand::Audio(Zeroizing::new(vec![1; 3_200])))
            .await
            .unwrap();
        let (accepted_tx, accepted_rx) = oneshot::channel();
        command_tx
            .send(SessionCommand::Commit {
                reason: CommitReason::Silence,
                accepted: accepted_tx,
            })
            .await
            .unwrap();
        accepted_rx.await.unwrap().unwrap();
        inbound
            .send(NativeInbound::Provider(
                super::super::harness_stream::ProviderEvent::Final {
                    session_id: "session_test".to_string(),
                    utterance_id: "utterance_test".to_string(),
                    revision: 1,
                    start_sample: 0,
                    end_sample: 1_600,
                    text: "hello".to_string(),
                    language: Some("en".to_string()),
                },
            ))
            .await
            .unwrap();
        tokio::task::yield_now().await;
        let (completed_tx, completed_rx) = oneshot::channel();
        command_tx
            .send(SessionCommand::Stop {
                finalize_current: true,
                accepted: completed_tx,
            })
            .await
            .unwrap();
        completed_rx.await.unwrap().unwrap();
        task.await.unwrap();
        let writes = writes.lock().unwrap();
        assert_eq!(writes[0], "audio:3200");
        assert!(writes[1].starts_with("commit:utterance_test:voice_asr_utterance_"));
        assert_eq!(writes.last().unwrap(), "stop:session_test");
        let types = captured
            .lock()
            .unwrap()
            .iter()
            .map(|value| value["type"].as_str().unwrap().to_string())
            .collect::<Vec<_>>();
        assert_eq!(types, vec!["final".to_string(), "stopped".to_string()]);
    }

    #[tokio::test]
    async fn fifty_millisecond_final_boundaries_have_no_loss_or_duplicates() {
        let (event, captured) = event_channel();
        let writes = Arc::new(Mutex::new(Vec::new()));
        let (native, inbound) = fake_connection(Box::new(RecordingSink(writes.clone())));
        let (command_tx, command_rx) = mpsc::channel(32);
        let task = tokio::spawn(run(config(event, Some(native)), command_rx));
        let mut utterance_id = "utterance_test".to_string();

        for index in 0..10 {
            command_tx
                .send(SessionCommand::Audio(Zeroizing::new(vec![1; 3_200])))
                .await
                .unwrap();
            let (accepted_tx, accepted_rx) = oneshot::channel();
            command_tx
                .send(SessionCommand::Commit {
                    reason: CommitReason::Silence,
                    accepted: accepted_tx,
                })
                .await
                .unwrap();
            accepted_rx.await.unwrap().unwrap();
            let next_id = writes
                .lock()
                .unwrap()
                .last()
                .unwrap()
                .split(':')
                .nth(2)
                .unwrap()
                .to_string();
            let final_event = super::super::harness_stream::ProviderEvent::Final {
                session_id: "session_test".to_string(),
                utterance_id: utterance_id.clone(),
                revision: 1,
                start_sample: 0,
                end_sample: if index % 2 == 0 { 800 } else { 1_600 },
                text: format!("fixture {index}"),
                language: Some("en".to_string()),
            };
            inbound
                .send(NativeInbound::Provider(final_event))
                .await
                .unwrap();
            inbound
                .send(NativeInbound::Provider(
                    super::super::harness_stream::ProviderEvent::Final {
                        session_id: "session_test".to_string(),
                        utterance_id: utterance_id.clone(),
                        revision: 1,
                        start_sample: 0,
                        end_sample: 1_600,
                        text: "duplicate".to_string(),
                        language: Some("en".to_string()),
                    },
                ))
                .await
                .unwrap();
            tokio::time::timeout(Duration::from_secs(1), async {
                loop {
                    let finals = captured
                        .lock()
                        .unwrap()
                        .iter()
                        .filter(|value| value["type"] == "final")
                        .count();
                    if finals == index + 1 {
                        break;
                    }
                    tokio::task::yield_now().await;
                }
            })
            .await
            .unwrap();
            utterance_id = next_id;
        }

        let (accepted_tx, accepted_rx) = oneshot::channel();
        command_tx
            .send(SessionCommand::Stop {
                finalize_current: false,
                accepted: accepted_tx,
            })
            .await
            .unwrap();
        accepted_rx.await.unwrap().unwrap();
        task.await.unwrap();
        let values = captured.lock().unwrap();
        let finals = values
            .iter()
            .filter(|value| value["type"] == "final")
            .collect::<Vec<_>>();
        assert_eq!(finals.len(), 10);
        assert!(finals
            .iter()
            .enumerate()
            .all(|(index, value)| value["text"] == format!("fixture {index}")));
    }

    async fn run_pcm_fixture(gate: SpeakerGate, packet_count: usize) -> Vec<Vec<u8>> {
        let (event, _) = event_channel();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let config = SessionConfig {
            session_id: "session_pcm".to_string(),
            current_utterance_id: "utterance_pcm".to_string(),
            event: VoiceAsrAuditChannel::plain(event),
            batch_decoder: Arc::new(RecordingDecoder { seen: seen.clone() }),
            native: None,
            speaker_gate: gate,
            allowed_languages: vec!["en".to_string()],
            final_timeout: FINAL_TIMEOUT,
            cancellation: Arc::new(RunCancellation::default()),
            start_degraded: None,
        };
        let (command_tx, command_rx) = mpsc::channel(32);
        let task = tokio::spawn(run(config, command_rx));
        let packet = 5_000_i16.to_le_bytes().repeat(1_600);
        for _ in 0..packet_count {
            command_tx
                .send(SessionCommand::Audio(Zeroizing::new(packet.clone())))
                .await
                .unwrap();
        }
        let (accepted_tx, accepted_rx) = oneshot::channel();
        command_tx
            .send(SessionCommand::Commit {
                reason: CommitReason::Silence,
                accepted: accepted_tx,
            })
            .await
            .unwrap();
        accepted_rx.await.unwrap().unwrap();
        let (completed_tx, completed_rx) = oneshot::channel();
        command_tx
            .send(SessionCommand::Stop {
                finalize_current: true,
                accepted: completed_tx,
            })
            .await
            .unwrap();
        completed_rx.await.unwrap().unwrap();
        task.await.unwrap();
        Arc::try_unwrap(seen).unwrap().into_inner().unwrap()
    }

    #[tokio::test]
    async fn all_speakers_preserves_pcm_and_target_rejection_never_leaks_raw_pattern() {
        let all = run_pcm_fixture(SpeakerGate::new(None, 0.001), 5).await;
        assert_eq!(all.len(), 1);
        let expected = 5_000_i16.to_le_bytes();
        assert!(all[0].chunks_exact(2).all(|sample| sample == expected));

        let target =
            run_pcm_fixture(SpeakerGate::new(Some(Arc::new(RejectScorer)), 0.001), 15).await;
        assert_eq!(target.len(), 1);
        assert!(target[0].iter().all(|byte| *byte == 0));
    }

    #[test]
    fn three_consecutive_batch_failures_are_fatal() {
        assert_eq!(next_failure(0), (1, false));
        assert_eq!(next_failure(1), (2, false));
        assert_eq!(next_failure(2), (3, true));
    }

    #[test]
    fn completed_decode_cancellations_do_not_accumulate_for_the_session_lifetime() {
        let completed = Arc::new(RunCancellation::default());
        let mut tracked = vec![Arc::downgrade(&completed)];
        drop(completed);
        let active = Arc::new(RunCancellation::default());

        track_decode_cancellation(&mut tracked, &active);
        track_decode_cancellation(&mut tracked, &active);

        assert_eq!(tracked.len(), 1);
        assert!(tracked[0].upgrade().is_some());
    }
}
