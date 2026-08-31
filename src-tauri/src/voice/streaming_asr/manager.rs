use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, Mutex, Weak,
    },
    time::Duration,
};

use tokio::sync::{mpsc, oneshot};
use zeroize::Zeroizing;

use super::{
    contracts::{CommitReason, VoiceAsrStreamEvent},
    session::{self, SessionCommand, SessionConfig},
};
use crate::{validate_identifier, RunCancellation};

const PACKET_BYTES: usize = 3_200;
const PACKET_SAMPLES: usize = 1_600;
const MAX_UTTERANCE_BYTES: usize = 960_000;
const COMMAND_CAPACITY: usize = 32;
const STOP_ACK_TIMEOUT: Duration = Duration::from_secs(2);

struct SessionHandle {
    generation: u64,
    next_sequence: u64,
    utterance_bytes: usize,
    sender: Option<mpsc::Sender<SessionCommand>>,
    cancellation: Arc<RunCancellation>,
}

struct Inner {
    sessions: Mutex<HashMap<String, SessionHandle>>,
    starting: AtomicBool,
    generation: AtomicU64,
}

impl Default for Inner {
    fn default() -> Self {
        Self {
            sessions: Mutex::new(HashMap::new()),
            starting: AtomicBool::new(false),
            generation: AtomicU64::new(0),
        }
    }
}

#[derive(Clone, Default)]
pub(crate) struct AsrSessionManager {
    inner: Arc<Inner>,
}

pub(crate) struct StartReservation {
    session_id: String,
    generation: u64,
    pub(crate) cancellation: Arc<RunCancellation>,
    inner: Weak<Inner>,
    armed: bool,
}

impl Drop for StartReservation {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        self.cancellation.cancel();
        if let Some(inner) = self.inner.upgrade() {
            if let Ok(mut sessions) = inner.sessions.lock() {
                if sessions
                    .get(&self.session_id)
                    .is_some_and(|handle| handle.generation == self.generation)
                {
                    sessions.remove(&self.session_id);
                }
            }
            inner.starting.store(false, Ordering::SeqCst);
        }
    }
}

impl AsrSessionManager {
    pub(crate) fn reserve(
        &self,
        session_id: String,
        conversation_id: &str,
        sample_rate: u32,
        recover_existing: bool,
    ) -> Result<StartReservation, String> {
        validate_identifier(&session_id, "ASR session id")?;
        validate_identifier(conversation_id, "conversation id")?;
        if sample_rate != 16_000 {
            return Err("ASR requires the canonical 16 kHz sample rate".to_string());
        }
        if self
            .inner
            .starting
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            return Err("asr-session-exists".to_string());
        }
        let result = (|| {
            let mut sessions = self
                .inner
                .sessions
                .lock()
                .map_err(|_| "ASR session manager unavailable".to_string())?;
            if !sessions.is_empty() && !recover_existing {
                return Err("asr-session-exists".to_string());
            }
            if recover_existing {
                for (_, stale) in sessions.drain() {
                    stale.cancellation.cancel();
                }
            }
            let generation = self.inner.generation.fetch_add(1, Ordering::SeqCst) + 1;
            let cancellation = Arc::new(RunCancellation::default());
            sessions.insert(
                session_id.clone(),
                SessionHandle {
                    generation,
                    next_sequence: 0,
                    utterance_bytes: 0,
                    sender: None,
                    cancellation: cancellation.clone(),
                },
            );
            Ok(StartReservation {
                session_id,
                generation,
                cancellation,
                inner: Arc::downgrade(&self.inner),
                armed: true,
            })
        })();
        if result.is_err() {
            self.inner.starting.store(false, Ordering::SeqCst);
        }
        result
    }

    pub(crate) fn install(
        &self,
        mut reservation: StartReservation,
        protocol: &'static str,
        scope: &'static str,
        config: SessionConfig,
    ) -> Result<(), String> {
        let (sender, receiver) = mpsc::channel(COMMAND_CAPACITY);
        let installed = (|| {
            let mut sessions = self
                .inner
                .sessions
                .lock()
                .map_err(|_| "ASR session manager unavailable".to_string())?;
            let handle = sessions
                .get_mut(&reservation.session_id)
                .filter(|handle| handle.generation == reservation.generation)
                .ok_or_else(|| "asr-session-not-found".to_string())?;
            if handle.cancellation.is_cancelled() {
                return Err("asr-cancelled".to_string());
            }
            handle.sender = Some(sender);
            Ok::<(), String>(())
        })();
        installed?;
        if config
            .event
            .send(VoiceAsrStreamEvent::Ready {
                session_id: reservation.session_id.clone(),
                current_utterance_id: config.current_utterance_id.clone(),
                protocol,
                scope,
            })
            .is_err()
        {
            return Err("asr-session-not-found".to_string());
        }
        let inner = self.inner.clone();
        let session_id = reservation.session_id.clone();
        let generation = reservation.generation;
        reservation.armed = false;
        tokio::spawn(async move {
            session::run(config, receiver).await;
            if let Ok(mut sessions) = inner.sessions.lock() {
                if sessions
                    .get(&session_id)
                    .is_some_and(|handle| handle.generation == generation)
                {
                    sessions.remove(&session_id);
                }
            }
        });
        self.inner.starting.store(false, Ordering::SeqCst);
        Ok(())
    }

    pub(crate) fn abort(&self, reservation: StartReservation) {
        drop(reservation);
    }

    pub(crate) fn append(
        &self,
        session_id: &str,
        sequence: u64,
        sample_count: usize,
        bytes: &[u8],
    ) -> Result<(), String> {
        if sample_count != PACKET_SAMPLES || bytes.len() != PACKET_BYTES {
            return Err("asr-packet-format".to_string());
        }
        let mut sessions = self
            .inner
            .sessions
            .lock()
            .map_err(|_| "ASR session manager unavailable".to_string())?;
        let handle = sessions
            .get_mut(session_id)
            .ok_or_else(|| "asr-session-not-found".to_string())?;
        if sequence != handle.next_sequence {
            return Err("asr-packet-sequence".to_string());
        }
        if handle.utterance_bytes.saturating_add(bytes.len()) > MAX_UTTERANCE_BYTES {
            return Err("asr-backpressure".to_string());
        }
        let sender = handle
            .sender
            .as_ref()
            .ok_or_else(|| "asr-session-not-found".to_string())?;
        sender
            .try_send(SessionCommand::Audio(Zeroizing::new(bytes.to_vec())))
            .map_err(|error| match error {
                mpsc::error::TrySendError::Full(_) => "asr-backpressure".to_string(),
                mpsc::error::TrySendError::Closed(_) => "asr-session-not-found".to_string(),
            })?;
        handle.next_sequence += 1;
        handle.utterance_bytes += bytes.len();
        Ok(())
    }

    pub(crate) async fn commit(
        &self,
        session_id: &str,
        reason: CommitReason,
    ) -> Result<(), String> {
        let (sender, generation) = self.sender(session_id)?;
        let (accepted_tx, accepted_rx) = oneshot::channel();
        sender
            .try_send(SessionCommand::Commit {
                reason,
                accepted: accepted_tx,
            })
            .map_err(|error| match error {
                mpsc::error::TrySendError::Full(_) => "asr-backpressure".to_string(),
                mpsc::error::TrySendError::Closed(_) => "asr-session-not-found".to_string(),
            })?;
        let result = accepted_rx
            .await
            .map_err(|_| "asr-session-not-found".to_string())?;
        if result.is_ok() {
            let mut sessions = self
                .inner
                .sessions
                .lock()
                .map_err(|_| "ASR session manager unavailable".to_string())?;
            if let Some(handle) = sessions
                .get_mut(session_id)
                .filter(|handle| handle.generation == generation)
            {
                handle.utterance_bytes = 0;
            }
        }
        result
    }

    pub(crate) async fn stop(&self, session_id: &str, finalize: bool) -> Result<(), String> {
        let (sender, cancellation, generation) = {
            let sessions = self
                .inner
                .sessions
                .lock()
                .map_err(|_| "ASR session manager unavailable".to_string())?;
            let handle = sessions
                .get(session_id)
                .ok_or_else(|| "asr-session-not-found".to_string())?;
            (
                handle.sender.clone(),
                handle.cancellation.clone(),
                handle.generation,
            )
        };
        let Some(sender) = sender else {
            cancellation.cancel();
            return Err("asr-session-not-found".to_string());
        };
        let (accepted_tx, accepted_rx) = oneshot::channel();
        let deadline = tokio::time::Instant::now() + STOP_ACK_TIMEOUT;
        if !matches!(
            tokio::time::timeout_at(
                deadline,
                sender.send(SessionCommand::Stop {
                    finalize_current: finalize,
                    accepted: accepted_tx,
                }),
            )
            .await,
            Ok(Ok(()))
        ) {
            cancellation.cancel();
            if let Ok(mut sessions) = self.inner.sessions.lock() {
                if sessions
                    .get(session_id)
                    .is_some_and(|handle| handle.generation == generation)
                {
                    sessions.remove(session_id);
                }
            }
            return Err("asr-final-timeout".to_string());
        }
        match tokio::time::timeout_at(deadline, accepted_rx).await {
            Ok(Ok(result)) => result,
            _ => {
                cancellation.cancel();
                if let Ok(mut sessions) = self.inner.sessions.lock() {
                    if sessions
                        .get(session_id)
                        .is_some_and(|handle| handle.generation == generation)
                    {
                        sessions.remove(session_id);
                    }
                }
                Err("asr-final-timeout".to_string())
            }
        }
    }

    pub(crate) fn shutdown(&self) {
        self.inner.starting.store(false, Ordering::SeqCst);
        if let Ok(mut sessions) = self.inner.sessions.lock() {
            for (_, session) in sessions.drain() {
                session.cancellation.cancel();
            }
        }
    }

    fn sender(&self, session_id: &str) -> Result<(mpsc::Sender<SessionCommand>, u64), String> {
        let sessions = self
            .inner
            .sessions
            .lock()
            .map_err(|_| "ASR session manager unavailable".to_string())?;
        sessions
            .get(session_id)
            .and_then(|handle| {
                handle
                    .sender
                    .clone()
                    .map(|sender| (sender, handle.generation))
            })
            .ok_or_else(|| "asr-session-not-found".to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn duplicate_start_is_rejected_and_recovery_can_replace_it() {
        let manager = AsrSessionManager::default();
        let first = manager
            .reserve("session_first".into(), "conversation_main", 16_000, false)
            .unwrap();
        assert_eq!(
            manager
                .reserve("session_second".into(), "conversation_main", 16_000, false)
                .err(),
            Some("asr-session-exists".to_string())
        );
        manager.abort(first);
        let replacement = manager
            .reserve("session_second".into(), "conversation_main", 16_000, true)
            .unwrap();
        manager.abort(replacement);
    }

    #[test]
    fn dropped_start_reservation_releases_the_single_start_guard() {
        let manager = AsrSessionManager::default();
        let abandoned = manager
            .reserve(
                "session_abandoned".into(),
                "conversation_main",
                16_000,
                false,
            )
            .unwrap();
        drop(abandoned);
        let replacement = manager
            .reserve(
                "session_replacement".into(),
                "conversation_main",
                16_000,
                false,
            )
            .unwrap();
        manager.abort(replacement);
    }

    #[test]
    fn canonical_packet_shape_is_enforced_before_session_lookup() {
        let manager = AsrSessionManager::default();
        assert_eq!(
            manager.append("missing", 0, PACKET_SAMPLES - 1, &[0; PACKET_BYTES]),
            Err("asr-packet-format".to_string())
        );
    }

    #[test]
    fn sequence_advances_only_after_the_bounded_queue_accepts_audio() {
        let manager = AsrSessionManager::default();
        let (sender, mut receiver) = mpsc::channel(1);
        manager.inner.sessions.lock().unwrap().insert(
            "session_test".to_string(),
            SessionHandle {
                generation: 1,
                next_sequence: 0,
                utterance_bytes: 0,
                sender: Some(sender),
                cancellation: Arc::new(RunCancellation::default()),
            },
        );
        let packet = [0; PACKET_BYTES];
        assert!(manager
            .append("session_test", 0, PACKET_SAMPLES, &packet)
            .is_ok());
        assert_eq!(
            manager.append("session_test", 0, PACKET_SAMPLES, &packet),
            Err("asr-packet-sequence".to_string())
        );
        assert_eq!(
            manager.append("session_test", 1, PACKET_SAMPLES, &packet),
            Err("asr-backpressure".to_string())
        );
        assert!(receiver.try_recv().is_ok());
        assert!(manager
            .append("session_test", 1, PACKET_SAMPLES, &packet)
            .is_ok());
    }

    #[tokio::test]
    async fn old_commit_ack_never_resets_recovered_session_capacity() {
        let manager = AsrSessionManager::default();
        let (old_sender, mut old_receiver) = mpsc::channel(1);
        manager.inner.sessions.lock().unwrap().insert(
            "same_session".to_string(),
            SessionHandle {
                generation: 1,
                next_sequence: 0,
                utterance_bytes: PACKET_BYTES,
                sender: Some(old_sender),
                cancellation: Arc::new(RunCancellation::default()),
            },
        );
        let commit_manager = manager.clone();
        let commit = tokio::spawn(async move {
            commit_manager
                .commit("same_session", CommitReason::Silence)
                .await
        });
        let Some(SessionCommand::Commit { accepted, .. }) = old_receiver.recv().await else {
            panic!("old session receives commit");
        };
        let (new_sender, _new_receiver) = mpsc::channel(1);
        manager.inner.sessions.lock().unwrap().insert(
            "same_session".to_string(),
            SessionHandle {
                generation: 2,
                next_sequence: 4,
                utterance_bytes: PACKET_BYTES * 4,
                sender: Some(new_sender),
                cancellation: Arc::new(RunCancellation::default()),
            },
        );
        accepted.send(Ok(())).unwrap();
        commit.await.unwrap().unwrap();
        assert_eq!(
            manager.inner.sessions.lock().unwrap()["same_session"].utterance_bytes,
            PACKET_BYTES * 4
        );
    }

    #[test]
    fn utterance_capacity_accepts_exactly_thirty_seconds() {
        let manager = AsrSessionManager::default();
        let (sender, _receiver) = mpsc::channel(301);
        manager.inner.sessions.lock().unwrap().insert(
            "session_capacity".to_string(),
            SessionHandle {
                generation: 1,
                next_sequence: 0,
                utterance_bytes: 0,
                sender: Some(sender),
                cancellation: Arc::new(RunCancellation::default()),
            },
        );
        let packet = [0; PACKET_BYTES];
        for sequence in 0..300 {
            manager
                .append("session_capacity", sequence, PACKET_SAMPLES, &packet)
                .unwrap();
        }
        assert_eq!(
            manager.append("session_capacity", 300, PACKET_SAMPLES, &packet),
            Err("asr-backpressure".to_string())
        );
    }
}
