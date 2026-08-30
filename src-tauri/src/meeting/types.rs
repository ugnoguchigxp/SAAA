use serde::{Deserialize, Serialize};

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
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PreflightInput {
    pub microphone_device_id: String,
    pub system_audio_enabled: bool,
    pub stt_model: String,
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
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StartInput {
    pub session_id: String,
    pub microphone_device_id: String,
    pub microphone_enabled: bool,
    pub system_audio_enabled: bool,
    pub stt_model: String,
    pub translation_enabled: bool,
    pub persistence_mode: String,
}
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SegmentInput {
    pub session_id: String,
    pub capture_token: String,
    pub lane: MeetingLane,
    pub sequence: u64,
    #[serde(skip)]
    pub samples: Vec<f32>,
    pub audio_upload_id: String,
    pub sample_rate: u32,
    pub started_at_ms: u64,
    pub duration_ms: u32,
}
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewSegmentInput {
    pub run_id: String,
    #[serde(flatten)]
    pub segment: SegmentInput,
}
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SessionInput {
    pub session_id: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SegmentResult {
    pub accepted: bool,
    pub text: String,
    pub language: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum MeetingEvent {
    StateChanged {
        session_id: Option<String>,
        state: MeetingState,
    },
    TranscriptFinal {
        session_id: String,
        lane: MeetingLane,
        sequence: u64,
        text: String,
        language: Option<String>,
    },
    TranscriptPartial {
        session_id: String,
        lane: MeetingLane,
        sequence: u64,
        text: String,
        language: Option<String>,
    },
    Failed {
        session_id: Option<String>,
        code: String,
        message: String,
        recovery: String,
    },
}
