use serde::{Deserialize, Serialize};

pub const RULE_VERSION: &str = "mvp1-rules-v1";
pub const POLICY_VERSION: &str = "mvp2.5-shadow-v1";

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum SituationScene {
    Conversation,
    Meeting,
    Coding,
    Writing,
    Media,
    Focus,
    Solo,
    Unknown,
}

impl SituationScene {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Conversation => "CONVERSATION",
            Self::Meeting => "MEETING",
            Self::Coding => "CODING",
            Self::Writing => "WRITING",
            Self::Media => "MEDIA",
            Self::Focus => "FOCUS",
            Self::Solo => "SOLO",
            Self::Unknown => "UNKNOWN",
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CalibrationParameters {
    pub classification_min_confidence: u8,
    pub low_confidence_max: u8,
    pub enter_sample_count: u8,
    pub exit_sample_count: u8,
    pub cooldown_ms: u64,
    #[serde(default = "default_input_active_max_ms")]
    pub input_active_max_ms: u64,
    #[serde(default = "default_input_recent_max_ms")]
    pub input_recent_max_ms: u64,
}

impl Default for CalibrationParameters {
    fn default() -> Self {
        Self {
            classification_min_confidence: 70,
            low_confidence_max: 45,
            enter_sample_count: 3,
            exit_sample_count: 5,
            cooldown_ms: 10_000,
            input_active_max_ms: default_input_active_max_ms(),
            input_recent_max_ms: default_input_recent_max_ms(),
        }
    }
}

pub fn validate_calibration_parameters(value: &CalibrationParameters) -> Result<(), String> {
    if !(50..=95).contains(&value.classification_min_confidence)
        || value.low_confidence_max > 60
        || value.enter_sample_count == 0
        || value.enter_sample_count > 10
        || value.exit_sample_count == 0
        || value.exit_sample_count > 20
        || value.cooldown_ms > 60_000
        || !(5_000..=120_000).contains(&value.input_active_max_ms)
        || !(60_000..=1_800_000).contains(&value.input_recent_max_ms)
        || value.input_active_max_ms >= value.input_recent_max_ms
        || value.low_confidence_max >= value.classification_min_confidence
    {
        return Err("Invalid calibration parameters".to_string());
    }
    Ok(())
}

pub const fn default_input_active_max_ms() -> u64 {
    30_000
}

pub const fn default_input_recent_max_ms() -> u64 {
    300_000
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SituationRuntimeSettings {
    pub enabled: bool,
    pub sample_interval_ms: u64,
    pub calendar_enabled: bool,
    pub retention_days: u32,
    pub max_ledger_entries: u32,
    pub heartbeat_interval_ms: u64,
    pub sensitive_application_categories: bool,
}

impl Default for SituationRuntimeSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            sample_interval_ms: 2_000,
            calendar_enabled: false,
            retention_days: 7,
            max_ledger_entries: 10_000,
            heartbeat_interval_ms: 300_000,
            sensitive_application_categories: true,
        }
    }
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum SignalHealth {
    Ready,
    Disabled,
    PermissionDenied,
    Unsupported,
    Degraded,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum ForegroundCategory {
    Communication,
    Coding,
    Writing,
    Browser,
    Media,
    Sensitive,
    Other,
    Unknown,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum ConversationState {
    Idle,
    UserInput,
    ModelRunning,
    AgentRunning,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum MicrophoneState {
    Inactive,
    SaaaCapturing,
    SaaaTranscribing,
    ExternalActive,
    Unknown,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum AudioState {
    Silent,
    SaaaSpeaking,
    ExternalMedia,
    Unknown,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum CalendarState {
    Free,
    Busy,
    MeetingLikely,
    Unavailable,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum TimeBucket {
    Now,
    Within15m,
    Later,
    None,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum InputActivityState {
    Active,
    Recent,
    Idle,
    Unknown,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct InputActivitySignal {
    pub state: InputActivityState,
    pub health: SignalHealth,
}

impl Default for InputActivitySignal {
    fn default() -> Self {
        unsupported_input_activity()
    }
}

pub fn unsupported_input_activity() -> InputActivitySignal {
    InputActivitySignal {
        state: InputActivityState::Unknown,
        health: SignalHealth::Unsupported,
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ForegroundSignal {
    pub category: ForegroundCategory,
    pub health: SignalHealth,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ConversationSignal {
    pub state: ConversationState,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct MicrophoneSignal {
    pub state: MicrophoneState,
    pub health: SignalHealth,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AudioSignal {
    pub state: AudioState,
    pub health: SignalHealth,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CalendarSignal {
    pub state: CalendarState,
    pub time_bucket: TimeBucket,
    pub health: SignalHealth,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SignalSnapshot {
    pub sequence: u64,
    pub observed_at: String,
    pub foreground: ForegroundSignal,
    #[serde(default = "unsupported_input_activity")]
    pub input_activity: InputActivitySignal,
    pub conversation: ConversationSignal,
    pub microphone: MicrophoneSignal,
    pub audio: AudioSignal,
    pub calendar: CalendarSignal,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Evidence {
    pub code: String,
    pub weight: i32,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SituationState {
    pub scene: String,
    pub confidence: u8,
    pub user_attention: String,
    pub audio_environment: String,
    pub evidence: Vec<Evidence>,
    pub candidate_since: String,
    pub stable_since: String,
    pub updated_at: String,
    pub rule_version: String,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ShadowDecision {
    pub mode: String,
    pub proposed_attention: String,
    pub actual_execution: String,
    pub actual_presentation: String,
    pub reason_codes: Vec<String>,
    pub decided_at: String,
    pub policy_version: String,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SituationFeedback {
    pub verdict: String,
    pub impact: String,
    pub corrected_scene: Option<String>,
    pub reason_code: Option<String>,
    pub created_at: String,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SituationLedgerEntry {
    pub id: String,
    pub observed_at: String,
    pub state: SituationState,
    pub decision: ShadowDecision,
    pub signal_health: Vec<SignalHealthEntry>,
    pub entry_kind: String,
    pub feedback: Option<SituationFeedback>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SignalHealthEntry {
    pub source: String,
    pub health: SignalHealth,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SituationEvaluationSummary {
    pub total_entries: u64,
    pub accurate: u64,
    pub inaccurate: u64,
    pub unsure: u64,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct QualityWindowCounters {
    pub sample_count: u64,
    pub candidate_change_count: u64,
    pub stable_transition_count: u64,
    pub unknown_sample_count: u64,
    pub stale_owned_signal_count: u64,
    pub decision_ignore_count: u64,
    pub decision_observe_count: u64,
    pub decision_suggest_count: u64,
    pub decision_respond_count: u64,
    pub health_ready_count: u64,
    pub health_disabled_count: u64,
    pub health_permission_denied_count: u64,
    pub health_unsupported_count: u64,
    pub health_degraded_count: u64,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SituationQualityMetrics {
    pub sample_count: u64,
    pub flapping_rate: Option<f64>,
    pub stale_rate: Option<f64>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SituationRuntimeFailure {
    pub code: String,
    pub message: String,
    pub recovery: String,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SituationSnapshot {
    pub monitoring_enabled: bool,
    pub monitoring_active: bool,
    pub signals: SignalSnapshot,
    pub state: SituationState,
    pub decision: ShadowDecision,
    pub last_failure: Option<SituationRuntimeFailure>,
    pub history: Vec<SituationLedgerEntry>,
    pub evaluation: SituationEvaluationSummary,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SituationReviewSnapshot {
    pub active_profile: super::calibration::CalibrationProfile,
    pub quality: SituationQualityMetrics,
    pub feedback_queue: Vec<SituationLedgerEntry>,
    pub latest_run: Option<super::calibration::CalibrationRun>,
    pub candidates: Vec<super::calibration::CalibrationProfile>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OwnedSignalInput {
    pub conversation_state: ConversationState,
    pub microphone_state: MicrophoneState,
    pub audio_state: AudioState,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SituationFeedbackInput {
    pub ledger_id: String,
    pub verdict: String,
    pub impact: String,
    pub corrected_scene: Option<String>,
    pub reason_code: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum SituationEvent {
    SignalHealthChanged {
        source: String,
        health: SignalHealth,
    },
    CandidateChanged {
        state: SituationState,
    },
    StableStateChanged {
        entry: SituationLedgerEntry,
    },
    ShadowDecisionChanged {
        entry: SituationLedgerEntry,
    },
    MonitoringStopped {
        reason: String,
    },
    Failed {
        code: String,
        message: String,
        recovery: String,
    },
}

pub fn initial_signals(now: &str) -> SignalSnapshot {
    SignalSnapshot {
        sequence: 0,
        observed_at: now.to_string(),
        foreground: ForegroundSignal {
            category: ForegroundCategory::Unknown,
            health: SignalHealth::Disabled,
        },
        conversation: ConversationSignal {
            state: ConversationState::Idle,
        },
        microphone: MicrophoneSignal {
            state: MicrophoneState::Inactive,
            health: SignalHealth::Ready,
        },
        audio: AudioSignal {
            state: AudioState::Silent,
            health: SignalHealth::Ready,
        },
        calendar: CalendarSignal {
            state: CalendarState::Unavailable,
            time_bucket: TimeBucket::None,
            health: SignalHealth::Disabled,
        },
        input_activity: InputActivitySignal {
            state: InputActivityState::Unknown,
            health: SignalHealth::Disabled,
        },
    }
}

pub fn initial_state(now: &str) -> SituationState {
    SituationState {
        scene: "UNKNOWN".to_string(),
        confidence: 0,
        user_attention: "unknown".to_string(),
        audio_environment: "unknown".to_string(),
        evidence: Vec::new(),
        candidate_since: now.to_string(),
        stable_since: now.to_string(),
        updated_at: now.to_string(),
        rule_version: RULE_VERSION.to_string(),
    }
}

pub fn initial_decision(now: &str) -> ShadowDecision {
    ShadowDecision {
        mode: "shadow".to_string(),
        proposed_attention: "IGNORE".to_string(),
        actual_execution: "NONE".to_string(),
        actual_presentation: "SILENT".to_string(),
        reason_codes: vec!["safe-default".to_string()],
        decided_at: now.to_string(),
        policy_version: POLICY_VERSION.to_string(),
    }
}
