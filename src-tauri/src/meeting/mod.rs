use crate::{
    bounded_text, new_id, now_iso, redact_runtime_text, whisper_executable, write_whisper_wav,
    RunCancellation,
};
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    fs,
    path::PathBuf,
    process::{Command, Stdio},
    sync::Mutex,
};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum MeetingState {
    Idle,
    Preflight,
    Ready,
    Active,
    Paused,
    Stopping,
    Completed,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "kebab-case")]
pub enum MeetingLane {
    Microphone,
    SystemAudio,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Health {
    pub status: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MeetingCapabilities {
    pub microphone: bool,
    pub system_audio: bool,
    pub overlay: bool,
    pub translation: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MeetingSnapshot {
    pub session_id: Option<String>,
    pub state: MeetingState,
    pub capture_token: Option<String>,
    pub entries: usize,
    pub capabilities: MeetingCapabilities,
    pub error: Option<MeetingError>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MeetingError {
    pub code: String,
    pub message: String,
    pub recovery: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PreflightInput {
    pub microphone_device_id: String,
    pub system_audio_enabled: bool,
    pub stt_model_path: String,
    pub translation_enabled: bool,
}
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PreflightResult {
    pub state: MeetingState,
    pub microphone: Health,
    pub system_audio: Health,
    pub stt: Health,
    pub translation: Health,
    pub shipping_capabilities: MeetingCapabilities,
    pub blocking_errors: Vec<MeetingError>,
}
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StartInput {
    pub session_id: String,
    pub microphone_device_id: String,
    pub microphone_enabled: bool,
    pub system_audio_enabled: bool,
    pub stt_model_path: String,
    pub translation_enabled: bool,
    pub persistence_mode: String,
}
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SegmentInput {
    pub session_id: String,
    pub capture_token: String,
    pub lane: MeetingLane,
    pub sequence: u64,
    pub samples: Vec<f32>,
    pub sample_rate: u32,
    pub started_at_ms: u64,
    pub duration_ms: u32,
}
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionInput {
    pub session_id: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SegmentResult {
    pub accepted: bool,
    pub text: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum MeetingEvent {
    StateChanged {
        session_id: Option<String>,
        state: MeetingState,
    },
}

#[derive(Clone)]
struct Entry {
    lane: MeetingLane,
    sequence: u64,
    text: String,
    started_at_ms: u64,
    ended_at_ms: u64,
}
struct Session {
    id: String,
    token: Option<String>,
    model: PathBuf,
    entries: Vec<Entry>,
    next_sequences: HashMap<MeetingLane, u64>,
    error: Option<MeetingError>,
}
pub struct MeetingRuntime {
    inner: Mutex<RuntimeInner>,
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
    pub fn preflight(&self, input: &PreflightInput) -> Result<PreflightResult, String> {
        let mut r = self.inner.lock().map_err(|_| "Meeting lock unavailable")?;
        ensure(&r.state, &[MeetingState::Idle])?;
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
        let stt = match fs::canonicalize(&input.stt_model_path) {
            Ok(path) if path.is_file() => health("ready", "local-whisper"),
            _ => {
                errors.push(err(
                    "MEETING_STT_MODEL_MISSING",
                    "The local Whisper model is unavailable.",
                    "Select an existing local model file in Settings.",
                ));
                health("unavailable", "Model file missing")
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
        tts_active: bool,
        connection: &Mutex<Connection>,
    ) -> Result<MeetingSnapshot, String> {
        let model = fs::canonicalize(&input.stt_model_path)
            .map_err(|_| "MEETING_STT_MODEL_MISSING: Select an existing local model file.")?;
        if !model.is_file() {
            return Err("MEETING_STT_MODEL_MISSING: Model path is not a file.".into());
        }
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
        if tts_active {
            return Err("MEETING_POLICY_TTS_BLOCKED: Stop speech and retry.".into());
        }
        let mut r = self.inner.lock().map_err(|_| "Meeting lock unavailable")?;
        ensure(&r.state, &[MeetingState::Ready])?;
        if input.session_id.len() > 160 || input.session_id.is_empty() {
            return Err("Invalid session id".into());
        }
        let token = format!("mt_{}", new_id("token"));
        {
            let conn = connection.lock().map_err(|_| "Database lock unavailable")?;
            conn.execute("INSERT INTO meeting_sessions(id,status,microphone_enabled,system_audio_enabled,stt_provider_id,stt_model_label,translation_provider_id,persistence_mode,started_at) VALUES (?1,'active',?2,0,'local-whisper',?3,NULL,'discard',?4)",params![input.session_id,if input.microphone_enabled {1} else {0},model.file_name().and_then(|x|x.to_str()).unwrap_or("model"),now_iso()]).map_err(|e|e.to_string())?;
        }
        r.state = MeetingState::Active;
        r.session = Some(Session {
            id: input.session_id.clone(),
            token: Some(token),
            model,
            entries: Vec::new(),
            next_sequences: HashMap::new(),
            error: None,
        });
        Ok(snapshot(&r))
    }
    pub fn pause(&self, id: &str) -> Result<MeetingSnapshot, String> {
        self.transition(id, MeetingState::Paused)
    }
    pub fn resume(&self, id: &str) -> Result<MeetingSnapshot, String> {
        let mut r = self.inner.lock().map_err(|_| "Meeting lock unavailable")?;
        if r.session.as_ref().ok_or("MEETING_INVALID_STATE")?.id != id {
            return Err("MEETING_INVALID_STATE".into());
        }
        ensure(&r.state, &[MeetingState::Paused])?;
        let s = r.session.as_mut().expect("meeting session checked");
        s.token = Some(format!("mt_{}", new_id("token")));
        r.state = MeetingState::Active;
        Ok(snapshot(&r))
    }
    fn transition(&self, id: &str, to: MeetingState) -> Result<MeetingSnapshot, String> {
        let mut r = self.inner.lock().map_err(|_| "Meeting lock unavailable")?;
        if r.session.as_ref().ok_or("MEETING_INVALID_STATE")?.id != id {
            return Err("MEETING_INVALID_STATE".into());
        }
        if r.state == to {
            return Ok(snapshot(&r));
        }
        ensure(&r.state, &[MeetingState::Active])?;
        let s = r.session.as_mut().expect("meeting session checked");
        s.token = None;
        r.state = to;
        Ok(snapshot(&r))
    }
    pub fn stop(
        &self,
        id: &str,
        connection: &Mutex<Connection>,
    ) -> Result<MeetingSnapshot, String> {
        let mut r = self.inner.lock().map_err(|_| "Meeting lock unavailable")?;
        if r.session.as_ref().ok_or("MEETING_INVALID_STATE")?.id != id {
            return Err("MEETING_INVALID_STATE".into());
        }
        if r.state == MeetingState::Completed {
            return Ok(snapshot(&r));
        }
        if !matches!(r.state, MeetingState::Active | MeetingState::Paused) {
            return Err("MEETING_INVALID_STATE".into());
        }
        let s = r.session.as_mut().expect("meeting session checked");
        s.token = None;
        r.state = MeetingState::Completed;
        let conn = connection.lock().map_err(|_| "Database lock unavailable")?;
        conn.execute(
            "UPDATE meeting_sessions SET status='completed', ended_at=?1 WHERE id=?2",
            params![now_iso(), id],
        )
        .map_err(|e| e.to_string())?;
        Ok(snapshot(&r))
    }
    pub fn append(
        &self,
        input: &SegmentInput,
        cancellation: &RunCancellation,
    ) -> Result<(PathBuf, Vec<f32>), String> {
        let mut r = self.inner.lock().map_err(|_| "Meeting lock unavailable")?;
        let active = r.state == MeetingState::Active;
        let s = r.session.as_mut().ok_or("MEETING_INVALID_STATE")?;
        if !active || s.id != input.session_id || s.token.as_deref() != Some(&input.capture_token) {
            return Err("MEETING_INVALID_CAPTURE_TOKEN".into());
        };
        if input.lane != MeetingLane::Microphone {
            return Err("MEETING_SYSTEM_AUDIO_UNAVAILABLE".into());
        };
        if !(8_000..=96_000).contains(&input.sample_rate)
            || !(1_000..=15_000).contains(&input.duration_ms)
            || input.samples.len() > input.sample_rate as usize * 15
        {
            return Err("MEETING_INVALID_STATE: Invalid segment bounds".into());
        };
        let next = s.next_sequences.get(&input.lane).copied().unwrap_or(0);
        if input.sequence != next {
            return Err("MEETING_OUT_OF_ORDER_SEGMENT".into());
        };
        s.next_sequences.insert(input.lane.clone(), next + 1);
        let _ = cancellation;
        Ok((s.model.clone(), input.samples.clone()))
    }
    pub fn finish_segment(
        &self,
        input: &SegmentInput,
        text: String,
    ) -> Result<SegmentResult, String> {
        let mut r = self.inner.lock().map_err(|_| "Meeting lock unavailable")?;
        let active = r.state == MeetingState::Active;
        let s = r.session.as_mut().ok_or("MEETING_INVALID_STATE")?;
        if !active || s.id != input.session_id {
            return Err("MEETING_INVALID_STATE".into());
        };
        let text = bounded_text(&text, 8000);
        let total = s
            .entries
            .iter()
            .map(|e| e.text.chars().count())
            .sum::<usize>()
            + text.chars().count();
        if total > 200_000 {
            return Err("MEETING_BACKPRESSURE".into());
        };
        s.entries.push(Entry {
            lane: input.lane.clone(),
            sequence: input.sequence,
            text: text.clone(),
            started_at_ms: input.started_at_ms,
            ended_at_ms: input.started_at_ms + input.duration_ms as u64,
        });
        Ok(SegmentResult {
            accepted: true,
            text,
        })
    }
    pub fn save(
        &self,
        id: &str,
        connection: &Mutex<Connection>,
    ) -> Result<MeetingSnapshot, String> {
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
        let mut conn = connection.lock().map_err(|_| "Database lock unavailable")?;
        let tx = conn.transaction().map_err(|e| e.to_string())?;
        for e in &s.entries {
            tx.execute("INSERT INTO meeting_transcript_entries(id,session_id,lane,sequence,original_text,started_at_ms,ended_at_ms,created_at) VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",params![new_id("meeting_entry"),id,match e.lane {MeetingLane::Microphone=>"microphone",MeetingLane::SystemAudio=>"system-audio"},e.sequence,e.text,e.started_at_ms,e.ended_at_ms,now_iso()]).map_err(|e|e.to_string())?;
        }
        tx.execute(
            "UPDATE meeting_sessions SET status='saved', saved_at=?1 WHERE id=?2",
            params![now_iso(), id],
        )
        .map_err(|e| e.to_string())?;
        tx.commit().map_err(|e| e.to_string())?;
        r.state = MeetingState::Idle;
        r.session = None;
        Ok(snapshot(&r))
    }
    pub fn discard(&self, id: &str, connection: &Mutex<Connection>) -> Result<(), String> {
        let mut r = self.inner.lock().map_err(|_| "Meeting lock unavailable")?;
        let s = r.session.as_ref().ok_or("MEETING_INVALID_STATE")?;
        if s.id != id || !matches!(r.state, MeetingState::Completed | MeetingState::Failed) {
            return Err("MEETING_INVALID_STATE".into());
        };
        let conn = connection.lock().map_err(|_| "Database lock unavailable")?;
        conn.execute("UPDATE meeting_sessions SET status='discarded', ended_at=COALESCE(ended_at,?1) WHERE id=?2",params![now_iso(),id]).map_err(|e|e.to_string())?;
        r.state = MeetingState::Idle;
        r.session = None;
        Ok(())
    }
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

pub fn transcribe_segment(
    model: PathBuf,
    samples: Vec<f32>,
    sample_rate: u32,
    cancellation: &RunCancellation,
) -> Result<String, String> {
    let dir = tempfile::tempdir().map_err(|e| format!("Could not create temporary audio: {e}"))?;
    let wav = dir.path().join("meeting-segment.wav");
    let output = dir.path().join("meeting-segment");
    write_whisper_wav(&wav, &samples, sample_rate)?;
    let mut child = Command::new(whisper_executable()?)
        .args(["-m"])
        .arg(model)
        .arg("-f")
        .arg(&wav)
        .args(["-otxt", "-of"])
        .arg(&output)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("Could not start local whisper: {e}"))?;
    loop {
        if cancellation.is_cancelled() {
            let _ = child.kill();
            let _ = child.wait();
            return Err("Transcription cancelled".into());
        };
        if let Some(status) = child.try_wait().map_err(|e| e.to_string())? {
            if !status.success() {
                return Err(format!("Local whisper exited with {status}"));
            };
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(40));
    }
    let text = fs::read_to_string(output.with_extension("txt"))
        .map_err(|_| "Local whisper completed without a transcript".to_string())?;
    if text.trim().is_empty() {
        return Err("Local whisper completed without a transcript".into());
    };
    Ok(bounded_text(text.trim(), 8000))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preflight_rejects_missing_local_model_without_arming_capture() {
        let runtime = MeetingRuntime::new();
        let result = runtime
            .preflight(&PreflightInput {
                microphone_device_id: "default".into(),
                system_audio_enabled: false,
                stt_model_path: "/missing/model.bin".into(),
                translation_enabled: false,
            })
            .expect("preflight result");
        assert_eq!(result.state, MeetingState::Idle);
        assert_eq!(result.blocking_errors[0].code, "MEETING_STT_MODEL_MISSING");
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
        let runtime = MeetingRuntime {
            inner: Mutex::new(RuntimeInner {
                state: MeetingState::Paused,
                session: Some(Session {
                    id: "session_a".into(),
                    token: None,
                    model: PathBuf::from("model"),
                    entries: Vec::new(),
                    next_sequences: HashMap::new(),
                    error: None,
                }),
            }),
        };
        assert_eq!(runtime.pause("other").unwrap_err(), "MEETING_INVALID_STATE");
        assert_eq!(
            runtime.resume("other").unwrap_err(),
            "MEETING_INVALID_STATE"
        );
    }
}
