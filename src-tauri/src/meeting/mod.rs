pub(crate) mod commands;
mod types;
use crate::{
    bounded_text, new_id, now_iso, persistence::SqliteWriter, redact_runtime_text, RunCancellation,
};
use rusqlite::{params, Connection};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};
use tauri::ipc::Channel;
pub use types::*;
use uuid::Uuid;
use zeroize::Zeroizing;

#[derive(Clone)]
struct Entry {
    lane: MeetingLane,
    sequence: u64,
    text: String,
    language: Option<String>,
    started_at_ms: u64,
    ended_at_ms: u64,
}
struct Session {
    id: String,
    token: Option<String>,
    entries: Vec<Entry>,
    total_text_chars: usize,
    next_sequences: HashMap<MeetingLane, u64>,
    in_flight: HashMap<(MeetingLane, u64), Arc<RunCancellation>>,
    error: Option<MeetingError>,
}
pub struct MeetingRuntime {
    inner: Mutex<RuntimeInner>,
    subscribers: Mutex<HashMap<String, Channel<MeetingEvent>>>,
}
struct RuntimeInner {
    state: MeetingState,
    session: Option<Session>,
}

impl MeetingRuntime {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(RuntimeInner {
                state: MeetingState::Idle,
                session: None,
            }),
            subscribers: Mutex::new(HashMap::new()),
        }
    }
    pub fn watch(&self, subscriber_id: &str, channel: Channel<MeetingEvent>) -> Result<(), String> {
        crate::validate_identifier(subscriber_id, "meeting subscriber id")?;
        let runtime = self.inner.lock().map_err(|_| "Meeting lock unavailable")?;
        let snapshot = snapshot(&runtime);
        let mut subscribers = self
            .subscribers
            .lock()
            .map_err(|_| "Meeting event lock unavailable")?;
        if !subscribers.contains_key(subscriber_id) && subscribers.len() >= 32 {
            return Err("MEETING_BACKPRESSURE: Too many event subscribers".to_string());
        }
        channel
            .send(MeetingEvent::StateChanged {
                session_id: snapshot.session_id,
                state: snapshot.state,
            })
            .map_err(|_| "Meeting event channel is unavailable".to_string())?;
        subscribers.insert(subscriber_id.to_string(), channel);
        Ok(())
    }
    pub fn unwatch(&self, subscriber_id: &str) -> Result<(), String> {
        crate::validate_identifier(subscriber_id, "meeting subscriber id")?;
        self.subscribers
            .lock()
            .map_err(|_| "Meeting event lock unavailable")?
            .remove(subscriber_id);
        Ok(())
    }
    pub fn emit(&self, event: MeetingEvent) {
        if let Ok(mut subscribers) = self.subscribers.lock() {
            subscribers.retain(|_, channel| channel.send(event.clone()).is_ok());
        }
    }
    pub fn blocks_tts(&self) -> bool {
        self.inner
            .lock()
            .map(|r| {
                matches!(
                    r.state,
                    MeetingState::Active | MeetingState::Paused | MeetingState::Stopping
                )
            })
            .unwrap_or(true)
    }
    pub fn snapshot(&self) -> Result<MeetingSnapshot, String> {
        let r = self.inner.lock().map_err(|_| "Meeting lock unavailable")?;
        Ok(snapshot(&r))
    }
    pub fn preflight(
        &self,
        input: &PreflightInput,
        asr_health: Result<(), String>,
    ) -> Result<PreflightResult, String> {
        validate_device_bounds(&input.microphone_device_id)?;
        let mut r = self.inner.lock().map_err(|_| "Meeting lock unavailable")?;
        ensure(&r.state, &[MeetingState::Idle, MeetingState::Ready])?;
        r.state = MeetingState::Preflight;
        let mut errors = Vec::new();
        let microphone = if input.microphone_device_id.trim().is_empty() {
            errors.push(err(
                "MEETING_DEVICE_UNAVAILABLE",
                "A microphone device is required.",
                "Choose an available microphone and retry.",
            ));
            health("unavailable", "No microphone selected")
        } else {
            health("ready", "Microphone permission was checked by the app")
        };
        let stt = match asr_health {
            Ok(()) => health("ready", "Selected Voice ASR"),
            Err(error) => {
                errors.push(err(
                    "MEETING_STT_UNAVAILABLE",
                    &error,
                    "Check the ASR service selected in Voice settings and retry.",
                ));
                health("unavailable", "Selected ASR unavailable")
            }
        };
        if input.system_audio_enabled {
            errors.push(err(
                "MEETING_SYSTEM_AUDIO_UNAVAILABLE",
                "System audio is unavailable in this build.",
                "Continue with microphone-only capture.",
            ));
        }
        if input.translation_enabled {
            errors.push(err(
                "MEETING_TRANSLATION_UNAVAILABLE",
                "Translation is unavailable in this build.",
                "Continue with original transcript only.",
            ));
        }
        r.state = if errors.is_empty() {
            MeetingState::Ready
        } else {
            MeetingState::Idle
        };
        Ok(PreflightResult {
            state: r.state.clone(),
            microphone,
            system_audio: health("unavailable", "Unavailable in this build"),
            stt,
            translation: health("disabled", "Unavailable in this build"),
            shipping_capabilities: capabilities(),
            blocking_errors: errors,
        })
    }
    pub fn start(
        &self,
        input: &StartInput,
        connection: &SqliteWriter,
    ) -> Result<MeetingSnapshot, String> {
        validate_device_bounds(&input.microphone_device_id)?;
        crate::validate_identifier(&input.session_id, "meeting session id")?;
        if input.microphone_device_id.trim().is_empty()
            || !input.microphone_enabled
            || input.system_audio_enabled
            || input.translation_enabled
            || input.persistence_mode != "discard"
        {
            return Err(
                "MEETING_INVALID_STATE: This build supports microphone-only Discard sessions."
                    .into(),
            );
        }
        let mut r = self.inner.lock().map_err(|_| "Meeting lock unavailable")?;
        ensure(&r.state, &[MeetingState::Ready])?;
        let token = capture_token();
        connection.write(|conn| {
            conn.execute("INSERT INTO meeting_sessions(id,status,microphone_enabled,system_audio_enabled,stt_provider_id,stt_model_label,translation_provider_id,persistence_mode,started_at) VALUES (?1,'active',?2,0,?3,?4,NULL,'discard',?5)",params![input.session_id,if input.microphone_enabled {1} else {0},crate::voice::network_asr::PROVIDER_ID,crate::voice::network_asr::MODEL_ID,now_iso()]).map_err(crate::database_error)?;
            Ok(())
        })?;
        r.state = MeetingState::Active;
        r.session = Some(Session {
            id: input.session_id.clone(),
            token: Some(token),
            entries: Vec::new(),
            total_text_chars: 0,
            next_sequences: HashMap::new(),
            in_flight: HashMap::new(),
            error: None,
        });
        Ok(snapshot(&r))
    }
    pub fn pause(&self, id: &str, connection: &SqliteWriter) -> Result<MeetingSnapshot, String> {
        crate::validate_identifier(id, "meeting session id")?;
        let mut r = self.inner.lock().map_err(|_| "Meeting lock unavailable")?;
        matching_session(&r, id)?;
        if r.state == MeetingState::Paused {
            return Ok(snapshot(&r));
        }
        ensure(&r.state, &[MeetingState::Active])?;
        update_session_status(connection, id, "paused", None)?;
        let s = r.session.as_mut().ok_or("MEETING_INVALID_STATE")?;
        cancel_segments(s);
        s.token = None;
        r.state = MeetingState::Paused;
        Ok(snapshot(&r))
    }
    pub fn resume(&self, id: &str, connection: &SqliteWriter) -> Result<MeetingSnapshot, String> {
        crate::validate_identifier(id, "meeting session id")?;
        let mut r = self.inner.lock().map_err(|_| "Meeting lock unavailable")?;
        matching_session(&r, id)?;
        ensure(&r.state, &[MeetingState::Paused])?;
        update_session_status(connection, id, "active", None)?;
        let s = r.session.as_mut().ok_or("MEETING_INVALID_STATE")?;
        s.token = Some(capture_token());
        r.state = MeetingState::Active;
        Ok(snapshot(&r))
    }
    pub fn stop(&self, id: &str, connection: &SqliteWriter) -> Result<MeetingSnapshot, String> {
        crate::validate_identifier(id, "meeting session id")?;
        let mut r = self.inner.lock().map_err(|_| "Meeting lock unavailable")?;
        matching_session(&r, id)?;
        if r.state == MeetingState::Completed {
            return Ok(snapshot(&r));
        }
        if !matches!(r.state, MeetingState::Active | MeetingState::Paused) {
            return Err("MEETING_INVALID_STATE".into());
        }
        let ended_at = now_iso();
        update_session_status(connection, id, "completed", Some(&ended_at))?;
        let s = r.session.as_mut().ok_or("MEETING_INVALID_STATE")?;
        cancel_segments(s);
        s.token = None;
        r.state = MeetingState::Completed;
        Ok(snapshot(&r))
    }
    pub fn append(
        &self,
        input: &mut SegmentInput,
        cancellation: Arc<RunCancellation>,
    ) -> Result<Zeroizing<Vec<f32>>, String> {
        let mut r = self.inner.lock().map_err(|_| "Meeting lock unavailable")?;
        let active = r.state == MeetingState::Active;
        let s = r.session.as_mut().ok_or("MEETING_INVALID_STATE")?;
        if !active || s.id != input.session_id || s.token.as_deref() != Some(&input.capture_token) {
            return Err("MEETING_INVALID_CAPTURE_TOKEN".into());
        };
        if input.lane != MeetingLane::Microphone {
            return Err("MEETING_SYSTEM_AUDIO_UNAVAILABLE".into());
        };
        validate_segment_bounds(input)?;
        let next = s.next_sequences.get(&input.lane).copied().unwrap_or(0);
        if input.sequence != next {
            return Err("MEETING_OUT_OF_ORDER_SEGMENT".into());
        };
        let in_flight = s
            .in_flight
            .keys()
            .filter(|(lane, _)| lane == &input.lane)
            .count();
        if in_flight >= 2 {
            return Err("MEETING_BACKPRESSURE".into());
        }
        let following = next
            .checked_add(1)
            .ok_or_else(|| "MEETING_OUT_OF_ORDER_SEGMENT".to_string())?;
        s.next_sequences.insert(input.lane.clone(), following);
        s.in_flight
            .insert((input.lane.clone(), input.sequence), cancellation);
        Ok(Zeroizing::new(std::mem::take(&mut input.samples)))
    }
    pub fn preview(&self, input: &mut SegmentInput) -> Result<Zeroizing<Vec<f32>>, String> {
        let r = self.inner.lock().map_err(|_| "Meeting lock unavailable")?;
        let s = r.session.as_ref().ok_or("MEETING_INVALID_STATE")?;
        if r.state != MeetingState::Active
            || s.id != input.session_id
            || s.token.as_deref() != Some(&input.capture_token)
        {
            return Err("MEETING_INVALID_CAPTURE_TOKEN".into());
        }
        if input.lane != MeetingLane::Microphone {
            return Err("MEETING_SYSTEM_AUDIO_UNAVAILABLE".into());
        }
        validate_segment_bounds(input)?;
        let next = s.next_sequences.get(&input.lane).copied().unwrap_or(0);
        if input.sequence != next {
            return Err("MEETING_OUT_OF_ORDER_SEGMENT".into());
        }
        Ok(Zeroizing::new(std::mem::take(&mut input.samples)))
    }
    pub fn preview_is_current(&self, input: &SegmentInput) -> bool {
        let Ok(r) = self.inner.lock() else {
            return false;
        };
        let Some(s) = r.session.as_ref() else {
            return false;
        };
        r.state == MeetingState::Active
            && s.id == input.session_id
            && s.token.as_deref() == Some(&input.capture_token)
            && s.next_sequences.get(&input.lane).copied().unwrap_or(0) == input.sequence
    }
    pub fn finish_segment(
        &self,
        input: &SegmentInput,
        text: String,
        language: Option<String>,
    ) -> Result<SegmentResult, String> {
        let mut r = self.inner.lock().map_err(|_| "Meeting lock unavailable")?;
        let active = r.state == MeetingState::Active;
        let s = r
            .session
            .as_mut()
            .filter(|session| session.id == input.session_id)
            .ok_or("MEETING_INVALID_STATE")?;
        if s.in_flight
            .remove(&(input.lane.clone(), input.sequence))
            .is_none()
        {
            return Err("MEETING_INVALID_STATE".into());
        }
        if !active {
            return Err("MEETING_INVALID_STATE".into());
        };
        let text = bounded_text(&text, 8000);
        let total = s
            .total_text_chars
            .checked_add(text.chars().count())
            .ok_or_else(|| "MEETING_BACKPRESSURE".to_string())?;
        if s.entries.len() >= 2_000 || total > 200_000 {
            return Err("MEETING_BACKPRESSURE".into());
        };
        s.entries.push(Entry {
            lane: input.lane.clone(),
            sequence: input.sequence,
            text: text.clone(),
            language: language.clone(),
            started_at_ms: input.started_at_ms,
            ended_at_ms: input
                .started_at_ms
                .checked_add(u64::from(input.duration_ms))
                .ok_or_else(|| "MEETING_INVALID_STATE".to_string())?,
        });
        s.total_text_chars = total;
        Ok(SegmentResult {
            accepted: true,
            text,
            language,
        })
    }
    pub fn abort_segment(&self, input: &SegmentInput) {
        if let Ok(mut r) = self.inner.lock() {
            if let Some(session) = r
                .session
                .as_mut()
                .filter(|session| session.id == input.session_id)
            {
                session
                    .in_flight
                    .remove(&(input.lane.clone(), input.sequence));
            }
        }
    }
    pub fn fail(
        &self,
        id: &str,
        code: &str,
        message: &str,
        connection: &SqliteWriter,
    ) -> Result<(), String> {
        crate::validate_identifier(id, "meeting session id")?;
        validate_error_code(code)?;
        let error = err(
            code,
            message,
            "Discard the failed session and retry after resolving the issue.",
        );
        let mut runtime = self.inner.lock().map_err(|_| "Meeting lock unavailable")?;
        matching_session(&runtime, id)?;
        update_session_failure(connection, id, code, &now_iso())?;
        let session = runtime.session.as_mut().ok_or("MEETING_INVALID_STATE")?;
        cancel_segments(session);
        session.token = None;
        session.error = Some(error.clone());
        runtime.state = MeetingState::Failed;
        drop(runtime);
        self.emit(MeetingEvent::Failed {
            session_id: Some(id.to_string()),
            code: error.code,
            message: error.message,
            recovery: error.recovery,
        });
        Ok(())
    }
    pub fn shutdown(&self, connection: &SqliteWriter) {
        let session_id = if let Ok(mut runtime) = self.inner.lock() {
            if !matches!(
                runtime.state,
                MeetingState::Active | MeetingState::Paused | MeetingState::Stopping
            ) {
                return;
            }
            let id = runtime.session.as_ref().map(|session| session.id.clone());
            if let Some(session) = runtime.session.as_mut() {
                cancel_segments(session);
                session.token = None;
            }
            runtime.state = MeetingState::Idle;
            runtime.session = None;
            id
        } else {
            None
        };
        if let Some(id) = session_id {
            let _ = update_session_interrupted(connection, &id, &now_iso());
        }
    }
    pub fn save(&self, id: &str, connection: &SqliteWriter) -> Result<MeetingSnapshot, String> {
        crate::validate_identifier(id, "meeting session id")?;
        let mut r = self.inner.lock().map_err(|_| "Meeting lock unavailable")?;
        let completed = r.state == MeetingState::Completed;
        let s = r.session.as_mut().ok_or("MEETING_INVALID_STATE")?;
        if !completed || s.id != id {
            return Err("MEETING_INVALID_STATE".into());
        };
        if s.entries.len() > 2000
            || s.entries.iter().map(|e| e.text.len()).sum::<usize>() > 1_048_576
        {
            return Err("MEETING_SAVE_LIMIT_EXCEEDED".into());
        };
        connection.write(|conn| {
            let tx = conn.transaction().map_err(crate::database_error)?;
            for e in &s.entries {
                tx.execute("INSERT INTO meeting_transcript_entries(id,session_id,lane,sequence,original_text,original_language,started_at_ms,ended_at_ms,created_at) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9)",params![new_id("meeting_entry"),id,match e.lane {MeetingLane::Microphone=>"microphone",MeetingLane::SystemAudio=>"system-audio"},e.sequence,e.text,e.language.as_deref(),e.started_at_ms,e.ended_at_ms,now_iso()]).map_err(crate::database_error)?;
            }
            let changed = tx
                .execute(
                    "UPDATE meeting_sessions SET status='saved', saved_at=?1 WHERE id=?2",
                    params![now_iso(), id],
                )
                .map_err(crate::database_error)?;
            if changed != 1 {
                return Err("MEETING_INVALID_STATE".to_string());
            }
            tx.commit().map_err(crate::database_error)
        })?;
        r.state = MeetingState::Idle;
        r.session = None;
        Ok(snapshot(&r))
    }
    pub fn discard(&self, id: &str, connection: &SqliteWriter) -> Result<(), String> {
        crate::validate_identifier(id, "meeting session id")?;
        let mut r = self.inner.lock().map_err(|_| "Meeting lock unavailable")?;
        if r.session.is_none() && r.state == MeetingState::Idle {
            let discarded: bool = connection.read_serialized(|conn| {
                conn.query_row(
                        "SELECT EXISTS(SELECT 1 FROM meeting_sessions WHERE id=?1 AND status='discarded')",
                        [id],
                        |row| row.get(0),
                    )
                    .map_err(crate::database_error)
            })?;
            return if discarded {
                Ok(())
            } else {
                Err("MEETING_INVALID_STATE".into())
            };
        }
        let s = r.session.as_ref().ok_or("MEETING_INVALID_STATE")?;
        if s.id != id || !matches!(r.state, MeetingState::Completed | MeetingState::Failed) {
            return Err("MEETING_INVALID_STATE".into());
        };
        let changed = connection.write(|conn| {
            conn.execute("UPDATE meeting_sessions SET status='discarded', ended_at=COALESCE(ended_at,?1) WHERE id=?2",params![now_iso(),id]).map_err(crate::database_error)
        })?;
        if changed != 1 {
            return Err("MEETING_INVALID_STATE".to_string());
        }
        r.state = MeetingState::Idle;
        r.session = None;
        Ok(())
    }
}

fn validate_device_bounds(device_id: &str) -> Result<(), String> {
    if device_id.len() > 256 {
        return Err("Invalid microphone device id".to_string());
    }
    Ok(())
}

fn validate_segment_bounds(input: &SegmentInput) -> Result<(), String> {
    if !(8_000..=96_000).contains(&input.sample_rate)
        || !(1_000..=15_000).contains(&input.duration_ms)
        || input.samples.is_empty()
        || input.samples.len() > input.sample_rate as usize * 15
        || input.samples.iter().any(|sample| !sample.is_finite())
    {
        return Err("MEETING_INVALID_STATE: Invalid segment bounds".into());
    }
    let expected = u64::from(input.sample_rate) * u64::from(input.duration_ms) / 1_000;
    let actual = input.samples.len() as u64;
    let tolerance = u64::from(input.sample_rate / 50).max(256);
    if actual.abs_diff(expected) > tolerance
        || input
            .started_at_ms
            .checked_add(u64::from(input.duration_ms))
            .is_none()
    {
        return Err("MEETING_INVALID_STATE: Segment timing does not match samples".into());
    }
    Ok(())
}

fn capture_token() -> String {
    format!("mt_{}", Uuid::new_v4().simple())
}

fn matching_session<'a>(runtime: &'a RuntimeInner, id: &str) -> Result<&'a Session, String> {
    runtime
        .session
        .as_ref()
        .filter(|session| session.id == id)
        .ok_or_else(|| "MEETING_INVALID_STATE".to_string())
}

fn cancel_segments(session: &mut Session) {
    for cancellation in session.in_flight.values() {
        cancellation.cancel();
    }
    session.in_flight.clear();
}

fn update_session_status(
    connection: &SqliteWriter,
    id: &str,
    status: &str,
    ended_at: Option<&str>,
) -> Result<(), String> {
    connection.write(|conn| {
        let changed = conn
        .execute(
            "UPDATE meeting_sessions SET status=?1, ended_at=COALESCE(?2, ended_at) WHERE id=?3",
            params![status, ended_at, id],
        )
        .map_err(crate::database_error)?;
        if changed != 1 {
            return Err("MEETING_INVALID_STATE".to_string());
        }
        Ok(())
    })
}

fn update_session_failure(
    connection: &SqliteWriter,
    id: &str,
    code: &str,
    ended_at: &str,
) -> Result<(), String> {
    connection.write(|conn| {
        let changed = conn
            .execute(
                "UPDATE meeting_sessions
             SET status='failed', ended_at=COALESCE(ended_at, ?1), error_code=?2
             WHERE id=?3",
                params![ended_at, code, id],
            )
            .map_err(crate::database_error)?;
        if changed != 1 {
            return Err("MEETING_INVALID_STATE".to_string());
        }
        Ok(())
    })
}

fn update_session_interrupted(
    connection: &SqliteWriter,
    id: &str,
    ended_at: &str,
) -> Result<(), String> {
    connection.write(|conn| {
        let changed = conn
            .execute(
                "UPDATE meeting_sessions
             SET status='interrupted', ended_at=COALESCE(ended_at, ?1),
                 error_code=COALESCE(error_code, 'MEETING_INTERRUPTED')
             WHERE id=?2",
                params![ended_at, id],
            )
            .map_err(crate::database_error)?;
        if changed != 1 {
            return Err("MEETING_INVALID_STATE".to_string());
        }
        Ok(())
    })
}

fn validate_error_code(code: &str) -> Result<(), String> {
    if code.is_empty()
        || code.len() > 80
        || !code
            .chars()
            .all(|character| character.is_ascii_uppercase() || character == '_')
    {
        return Err("Invalid Meeting error code".to_string());
    }
    Ok(())
}

fn ensure(current: &MeetingState, allowed: &[MeetingState]) -> Result<(), String> {
    if allowed.contains(current) {
        Ok(())
    } else {
        Err("MEETING_INVALID_STATE".into())
    }
}
fn health(status: &str, message: &str) -> Health {
    Health {
        status: status.into(),
        message: message.into(),
    }
}
fn err(code: &str, message: &str, recovery: &str) -> MeetingError {
    MeetingError {
        code: code.into(),
        message: bounded_text(&redact_runtime_text(message), 500),
        recovery: bounded_text(recovery, 500),
    }
}
fn capabilities() -> MeetingCapabilities {
    MeetingCapabilities {
        microphone: true,
        system_audio: false,
        overlay: false,
        translation: false,
    }
}
fn snapshot(r: &RuntimeInner) -> MeetingSnapshot {
    let s = r.session.as_ref();
    MeetingSnapshot {
        session_id: s.map(|s| s.id.clone()),
        state: r.state.clone(),
        capture_token: s.and_then(|s| s.token.clone()),
        entries: s.map(|s| s.entries.len()).unwrap_or(0),
        capabilities: capabilities(),
        error: s.and_then(|s| s.error.clone()),
    }
}

pub fn reconcile(connection: &Connection) -> rusqlite::Result<()> {
    connection.execute("UPDATE meeting_sessions SET status='interrupted', ended_at=COALESCE(ended_at,?1), error_code=COALESCE(error_code,'MEETING_INTERRUPTED') WHERE status IN ('active','paused')",params![now_iso()])?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn meeting_events_serialize_camel_case_fields() {
        let event = serde_json::to_value(MeetingEvent::StateChanged {
            session_id: Some("session_contract".to_string()),
            state: MeetingState::Active,
        })
        .expect("meeting event serializes");
        assert_eq!(event["type"], "stateChanged");
        assert_eq!(event["sessionId"], "session_contract");
        assert!(event.get("session_id").is_none());
    }

    fn runtime_with_session(state: MeetingState) -> MeetingRuntime {
        MeetingRuntime {
            inner: Mutex::new(RuntimeInner {
                state,
                session: Some(Session {
                    id: "session_a".into(),
                    token: Some("capture_token".into()),
                    entries: Vec::new(),
                    total_text_chars: 0,
                    next_sequences: HashMap::new(),
                    in_flight: HashMap::new(),
                    error: None,
                }),
            }),
            subscribers: Mutex::new(HashMap::new()),
        }
    }

    fn meeting_connection(status: &str) -> SqliteWriter {
        let connection = Connection::open_in_memory().expect("in-memory database");
        connection
            .execute_batch(
                "CREATE TABLE meeting_sessions (
                    id TEXT PRIMARY KEY,
                    status TEXT NOT NULL,
                    ended_at TEXT,
                    error_code TEXT
                );",
            )
            .expect("meeting schema");
        connection
            .execute(
                "INSERT INTO meeting_sessions(id,status) VALUES('session_a',?1)",
                [status],
            )
            .expect("meeting session");
        SqliteWriter::from_connection(connection)
    }

    #[test]
    fn preflight_rejects_the_unavailable_selected_asr_without_arming_capture() {
        let runtime = MeetingRuntime::new();
        let result = runtime
            .preflight(
                &PreflightInput {
                    microphone_device_id: "default".into(),
                    system_audio_enabled: false,
                    translation_enabled: false,
                },
                Err("Selected ASR unavailable".into()),
            )
            .expect("preflight result");
        assert_eq!(result.state, MeetingState::Idle);
        assert_eq!(result.blocking_errors[0].code, "MEETING_STT_UNAVAILABLE");
        assert_eq!(
            runtime.snapshot().expect("snapshot").state,
            MeetingState::Idle
        );
    }

    #[test]
    fn active_meeting_policy_blocks_tts() {
        let runtime = MeetingRuntime {
            inner: Mutex::new(RuntimeInner {
                state: MeetingState::Active,
                session: None,
            }),
            subscribers: Mutex::new(HashMap::new()),
        };
        assert!(runtime.blocks_tts());
    }

    #[test]
    fn capabilities_keep_optional_features_disabled() {
        let value = capabilities();
        assert!(value.microphone);
        assert!(!value.system_audio && !value.overlay && !value.translation);
    }

    #[test]
    fn idempotent_pause_and_stop_do_not_accept_another_session_id() {
        let runtime = runtime_with_session(MeetingState::Paused);
        let connection = SqliteWriter::from_connection(
            Connection::open_in_memory().expect("in-memory database"),
        );
        assert_eq!(
            runtime.pause("other", &connection).unwrap_err(),
            "MEETING_INVALID_STATE"
        );
        assert_eq!(
            runtime.resume("other", &connection).unwrap_err(),
            "MEETING_INVALID_STATE"
        );
    }

    #[test]
    fn capture_tokens_are_random_128_bit_values() {
        let first = capture_token();
        let second = capture_token();
        assert_ne!(first, second);
        assert_eq!(first.len(), 35);
        assert!(first.starts_with("mt_"));
        assert!(first[3..]
            .chars()
            .all(|character| character.is_ascii_hexdigit()));
    }

    #[test]
    fn partial_preview_does_not_consume_sequence_and_is_dropped_after_final_starts() {
        let runtime = runtime_with_session(MeetingState::Active);
        let mut input = SegmentInput {
            session_id: "session_a".to_string(),
            capture_token: "capture_token".to_string(),
            lane: MeetingLane::Microphone,
            sequence: 0,
            samples: vec![0.0; 16_000],
            audio_upload_id: String::new(),
            sample_rate: 8_000,
            started_at_ms: 0,
            duration_ms: 2_000,
        };

        runtime.preview(&mut input).expect("preview is accepted");
        input.samples = vec![0.0; 16_000];
        assert!(runtime.preview_is_current(&input));
        runtime
            .append(&mut input, Arc::new(RunCancellation::default()))
            .expect("the same sequence remains available for the final segment");
        assert!(!runtime.preview_is_current(&input));
    }

    #[test]
    fn pause_updates_persisted_state_and_cancels_segments() {
        let runtime = runtime_with_session(MeetingState::Active);
        let cancellation = Arc::new(RunCancellation::default());
        runtime
            .inner
            .lock()
            .expect("meeting lock")
            .session
            .as_mut()
            .expect("session")
            .in_flight
            .insert((MeetingLane::Microphone, 0), cancellation.clone());
        let connection = meeting_connection("active");

        let paused = runtime.pause("session_a", &connection).expect("pause");

        assert_eq!(paused.state, MeetingState::Paused);
        assert!(paused.capture_token.is_none());
        assert!(cancellation.is_cancelled());
        let status: String = connection
            .lock()
            .expect("database lock")
            .query_row(
                "SELECT status FROM meeting_sessions WHERE id='session_a'",
                [],
                |row| row.get(0),
            )
            .expect("persisted status");
        assert_eq!(status, "paused");
        let input = SegmentInput {
            session_id: "session_a".to_string(),
            capture_token: "capture_token".to_string(),
            lane: MeetingLane::Microphone,
            sequence: 0,
            samples: vec![0.0; 8_000],
            audio_upload_id: String::new(),
            sample_rate: 8_000,
            started_at_ms: 0,
            duration_ms: 1_000,
        };
        assert!(runtime
            .finish_segment(&input, "late transcript".to_string(), None)
            .is_err());
        assert_eq!(
            runtime.snapshot().expect("snapshot").state,
            MeetingState::Paused
        );
        assert_eq!(
            runtime
                .pause("session_a", &connection)
                .expect("idempotent pause")
                .state,
            MeetingState::Paused
        );
    }

    #[test]
    fn failed_status_write_does_not_advance_runtime_state() {
        let runtime = runtime_with_session(MeetingState::Active);
        let connection = SqliteWriter::from_connection(
            Connection::open_in_memory().expect("in-memory database"),
        );

        assert!(runtime.pause("session_a", &connection).is_err());

        let snapshot = runtime.snapshot().expect("snapshot");
        assert_eq!(snapshot.state, MeetingState::Active);
        assert_eq!(snapshot.capture_token.as_deref(), Some("capture_token"));
    }

    #[test]
    fn meeting_failure_persists_only_a_bounded_error_code() {
        let runtime = runtime_with_session(MeetingState::Active);
        let connection = meeting_connection("active");

        runtime
            .fail(
                "session_a",
                "MEETING_STT_FAILED",
                "local transcription failed",
                &connection,
            )
            .expect("failure records");

        let (status, error_code): (String, String) = connection
            .lock()
            .expect("database lock")
            .query_row(
                "SELECT status,error_code FROM meeting_sessions WHERE id='session_a'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("failure metadata loads");
        assert_eq!(status, "failed");
        assert_eq!(error_code, "MEETING_STT_FAILED");
        assert_eq!(
            runtime.snapshot().expect("snapshot").state,
            MeetingState::Failed
        );
    }

    #[test]
    fn explicit_save_is_transactional_and_discard_keeps_transcript_memory_only() {
        let connection =
            SqliteWriter::from_connection(Connection::open_in_memory().expect("database opens"));
        crate::initialize_database(&connection.lock().expect("database lock"))
            .expect("schema initializes");
        {
            let conn = connection.lock().expect("database lock");
            conn.execute("INSERT INTO meeting_sessions(id,status,microphone_enabled,system_audio_enabled,stt_provider_id,stt_model_label,persistence_mode,started_at) VALUES('session_a','completed',1,0,'network-asr','model','discard','1')", []).expect("meeting fixture");
        }
        let runtime = runtime_with_session(MeetingState::Completed);
        runtime
            .inner
            .lock()
            .expect("meeting lock")
            .session
            .as_mut()
            .expect("session")
            .entries
            .push(Entry {
                lane: MeetingLane::Microphone,
                sequence: 0,
                text: "final text".into(),
                language: Some("Japanese".into()),
                started_at_ms: 0,
                ended_at_ms: 5_000,
            });
        runtime
            .save("session_a", &connection)
            .expect("save succeeds");
        let saved: i64 = connection
            .lock()
            .expect("database lock")
            .query_row(
                "SELECT COUNT(*) FROM meeting_transcript_entries WHERE session_id='session_a'",
                [],
                |row| row.get(0),
            )
            .expect("saved transcript count");
        assert_eq!(saved, 1);
        let saved_language: Option<String> = connection
            .lock()
            .expect("database lock")
            .query_row(
                "SELECT original_language FROM meeting_transcript_entries WHERE session_id='session_a'",
                [],
                |row| row.get(0),
            )
            .expect("saved transcript language");
        assert_eq!(saved_language.as_deref(), Some("Japanese"));

        {
            let conn = connection.lock().expect("database lock");
            conn.execute("INSERT INTO meeting_sessions(id,status,microphone_enabled,system_audio_enabled,stt_provider_id,stt_model_label,persistence_mode,started_at) VALUES('session_b','completed',1,0,'network-asr','model','discard','1')", []).expect("discard fixture");
        }
        let runtime = runtime_with_session(MeetingState::Completed);
        runtime
            .inner
            .lock()
            .expect("meeting lock")
            .session
            .as_mut()
            .expect("session")
            .id = "session_b".into();
        runtime
            .inner
            .lock()
            .expect("meeting lock")
            .session
            .as_mut()
            .expect("session")
            .entries
            .push(Entry {
                lane: MeetingLane::Microphone,
                sequence: 0,
                text: "discarded text".into(),
                language: None,
                started_at_ms: 0,
                ended_at_ms: 5_000,
            });
        runtime
            .discard("session_b", &connection)
            .expect("discard succeeds");
        let discarded: i64 = connection
            .lock()
            .expect("database lock")
            .query_row(
                "SELECT COUNT(*) FROM meeting_transcript_entries WHERE session_id='session_b'",
                [],
                |row| row.get(0),
            )
            .expect("discarded transcript count");
        assert_eq!(discarded, 0);
    }

    #[test]
    fn segment_validation_does_not_consume_sequence_and_backpressure_is_bounded() {
        let runtime = runtime_with_session(MeetingState::Active);
        let mut input = SegmentInput {
            session_id: "session_a".to_string(),
            capture_token: "capture_token".to_string(),
            lane: MeetingLane::Microphone,
            sequence: 0,
            samples: vec![0.0; 8_000],
            audio_upload_id: String::new(),
            sample_rate: 8_000,
            started_at_ms: 0,
            duration_ms: 1_000,
        };
        input.samples[0] = f32::NAN;
        assert!(runtime
            .append(&mut input, Arc::new(RunCancellation::default()))
            .is_err());

        input.samples[0] = 0.0;
        runtime
            .append(&mut input, Arc::new(RunCancellation::default()))
            .expect("first segment reserves");
        input.sequence = 1;
        input.started_at_ms = 1_000;
        input.samples = vec![0.0; 8_000];
        runtime
            .append(&mut input, Arc::new(RunCancellation::default()))
            .expect("second segment reserves");
        input.sequence = 2;
        input.started_at_ms = 2_000;
        input.samples = vec![0.0; 8_000];
        assert_eq!(
            runtime
                .append(&mut input, Arc::new(RunCancellation::default()))
                .unwrap_err(),
            "MEETING_BACKPRESSURE"
        );
    }

    #[test]
    fn meeting_module_has_no_network_or_automatic_action_calls() {
        let source = include_str!("mod.rs")
            .split("#[cfg(test)]")
            .next()
            .expect("production source exists");
        for forbidden in [
            "reqwest::",
            "start_turn",
            "speak_text",
            "codex_",
            "notification",
            "open_url",
            "open_path",
        ] {
            assert!(
                !source.contains(forbidden),
                "Meeting module must not contain {forbidden}"
            );
        }
    }
}
