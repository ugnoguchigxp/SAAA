use super::contracts::VoiceAsrStreamEvent;
use crate::validate_identifier;
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};
use tauri::ipc::Channel;
use zeroize::Zeroizing;
const PACKET_BYTES: usize = 3_200;
const PACKET_SAMPLES: usize = 1_600;
const MAX_UTTERANCE_BYTES: usize = 960_000;
struct Session {
    next_sequence: u64,
    utterance_id: String,
    samples: Zeroizing<Vec<u8>>,
    event: Channel<VoiceAsrStreamEvent>,
}
pub(crate) struct CommittedUtterance {
    pub(crate) session_id: String,
    pub(crate) utterance_id: String,
    pub(crate) bytes: Zeroizing<Vec<u8>>,
    pub(crate) event: Channel<VoiceAsrStreamEvent>,
}
#[derive(Clone, Default)]
pub(crate) struct AsrSessionManager {
    sessions: Arc<Mutex<HashMap<String, Session>>>,
}
impl AsrSessionManager {
    pub(crate) fn start(
        &self,
        session_id: String,
        conversation_id: String,
        sample_rate: u32,
        protocol: &'static str,
        event: Channel<VoiceAsrStreamEvent>,
    ) -> Result<(), String> {
        validate_identifier(&session_id, "ASR session id")?;
        validate_identifier(&conversation_id, "conversation id")?;
        if sample_rate != 16_000 {
            return Err("ASR requires the canonical 16 kHz sample rate".to_string());
        }
        let utterance_id = crate::new_id("voice_asr_utterance");
        let mut sessions = self
            .sessions
            .lock()
            .map_err(|_| "ASR session manager unavailable".to_string())?;
        if !sessions.is_empty() {
            return Err("asr-session-exists".to_string());
        }
        let _ = event.send(VoiceAsrStreamEvent::Ready {
            session_id: session_id.clone(),
            current_utterance_id: utterance_id.clone(),
            protocol,
            scope: "all-speakers",
        });
        sessions.insert(
            session_id,
            Session {
                next_sequence: 0,
                utterance_id,
                samples: Zeroizing::new(Vec::new()),
                event,
            },
        );
        Ok(())
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
            .sessions
            .lock()
            .map_err(|_| "ASR session manager unavailable".to_string())?;
        let session = sessions
            .get_mut(session_id)
            .ok_or_else(|| "asr-session-not-found".to_string())?;
        if sequence != session.next_sequence {
            return Err("asr-packet-sequence".to_string());
        }
        if session.samples.len() + bytes.len() > MAX_UTTERANCE_BYTES {
            return Err("asr-backpressure".to_string());
        }
        session.samples.extend_from_slice(bytes);
        session.next_sequence += 1;
        Ok(())
    }
    pub(crate) fn take_commit(&self, session_id: &str) -> Result<CommittedUtterance, String> {
        let mut sessions = self
            .sessions
            .lock()
            .map_err(|_| "ASR session manager unavailable".to_string())?;
        let session = sessions
            .get_mut(session_id)
            .ok_or_else(|| "asr-session-not-found".to_string())?;
        let bytes = std::mem::replace(&mut session.samples, Zeroizing::new(Vec::new()));
        let utterance_id = session.utterance_id.clone();
        session.utterance_id = crate::new_id("voice_asr_utterance");
        Ok(CommittedUtterance {
            session_id: session_id.to_string(),
            utterance_id,
            bytes,
            event: session.event.clone(),
        })
    }
    pub(crate) fn stop(&self, session_id: &str, _finalize: bool) -> Result<(), String> {
        let mut sessions = self
            .sessions
            .lock()
            .map_err(|_| "ASR session manager unavailable".to_string())?;
        let Some(mut session) = sessions.remove(session_id) else {
            return Err("asr-session-not-found".to_string());
        };
        session.samples.fill(0);
        let _ = session.event.send(VoiceAsrStreamEvent::Stopped {
            session_id: session_id.to_string(),
        });
        Ok(())
    }
    pub(crate) fn shutdown(&self) {
        if let Ok(mut sessions) = self.sessions.lock() {
            for (session_id, mut session) in sessions.drain() {
                session.samples.fill(0);
                let _ = session
                    .event
                    .send(VoiceAsrStreamEvent::Stopped { session_id });
            }
        }
    }
}
