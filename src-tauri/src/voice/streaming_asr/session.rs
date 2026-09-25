use super::{
    batch_engine::{BatchEngine, DecodeKind, DecodeRequest},
    batch_runtime::{BatchDecode, BatchDecodeOutcome},
    contracts::{CommitReason, VoiceAsrFailureCode, VoiceAsrStreamEvent},
    speaker_gate_runtime::SpeakerGate,
};
use crate::{persistence::audit::VoiceAsrAuditChannel, RunCancellation};
use std::{
    collections::VecDeque,
    sync::{Arc, Weak},
    time::Duration,
};
use tokio::sync::{mpsc, oneshot, Semaphore};
use zeroize::Zeroizing;

const FINAL_TIMEOUT: Duration = Duration::from_secs(15);
const RESULT_CAPACITY: usize = 8;
const TARGET_SILENCE_SAMPLES: u64 = 24_000;
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
    pub(crate) speaker_gate: SpeakerGate,
    pub(crate) cancellation: Arc<RunCancellation>,
}
struct Utterance {
    id: String,
    pcm: Zeroizing<Vec<u8>>,
    engine: BatchEngine,
    revision: u64,
    partial_cancellation: Arc<RunCancellation>,
}
impl Utterance {
    fn new(id: String) -> Self {
        Self {
            id,
            pcm: Zeroizing::new(Vec::new()),
            engine: BatchEngine::default(),
            revision: 0,
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
    gate: SpeakerGate,
    cancellation: Arc<RunCancellation>,
    current: Utterance,
    pending: VecDeque<Utterance>,
    batch_failures: usize,
    stopping: bool,
    stop_deadline: Option<tokio::time::Instant>,
    target_seen: bool,
    target_silence_samples: u64,
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
        gate: config.speaker_gate,
        cancellation: config.cancellation,
        current,
        pending: VecDeque::new(),
        batch_failures: 0,
        stopping: false,
        stop_deadline: None,
        target_seen: false,
        target_silence_samples: 0,
    };

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
                self.commit_after_target_silence().await;
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
            if self.gate.scope() == "target-speaker" {
                if packet.iter().any(|byte| *byte != 0) {
                    self.target_seen = true;
                    self.target_silence_samples = 0;
                } else if self.target_seen {
                    self.target_silence_samples += (packet.len() / 2) as u64;
                }
            }
            self.current.pcm.extend_from_slice(&packet);
            if let Some(request) = self.current.engine.on_audio(self.current.sample_count()) {
                self.spawn_decode(self.current.id.clone(), request, self.current.pcm.clone());
            }
        }
    }

    async fn commit_after_target_silence(&mut self) {
        if !self.target_seen || self.target_silence_samples < TARGET_SILENCE_SAMPLES {
            return;
        }
        // Resolve the gate's buffered lookahead before closing the segment. A new
        // target utterance in that buffer must keep the current segment open.
        let sanitized = self.gate.flush().await;
        self.process_sanitized(sanitized).await;
        if self.target_silence_samples < TARGET_SILENCE_SAMPLES {
            return;
        }
        if self.pending.len() >= 2 {
            return;
        }
        if let Err(error) = self.commit_current().await {
            self.emit_failed(
                Some(self.current.id.clone()),
                failure_code(&error),
                &error,
                false,
            );
        }
    }

    async fn commit_current(&mut self) -> Result<(), String> {
        self.target_seen = false;
        self.target_silence_samples = 0;
        let next_id = crate::new_id("voice_asr_utterance");
        let end_sample = self.current.sample_count();
        self.current.partial_cancellation.cancel();
        let mut committed = std::mem::replace(&mut self.current, Utterance::new(next_id.clone()));

        if end_sample == 0 {
            self.emit_discarded(&committed.id, "no-speech");
            return Ok(());
        }
        let request = committed.engine.commit(end_sample);
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
        if decoded.utterance_id != self.current.id {
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

    async fn check_deadlines(&mut self) -> bool {
        let now = tokio::time::Instant::now();
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
mod tests;
