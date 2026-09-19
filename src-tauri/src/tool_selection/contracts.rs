//! Configuration, request context, error envelope, semantic labels and limits for the
//! tool-selection D0–D3 milestone.
//!
//! Everything here is pure data plus parsing. No SQL and no inference live in this module so the
//! same types can be used by tests, the gateway and the L-Lang backend adapter.

use serde::{Deserialize, Serialize};
use std::fmt;
use std::path::{Path, PathBuf};

/// Environment variable that points at the tool-selection configuration file.
pub const CONFIG_ENV: &str = "SAAA_TOOL_SELECTION_CONFIG";
pub const CONFIG_FORMAT_VERSION: u32 = 1;
pub const CONFIG_MAX_BYTES: u64 = 64 * 1024;

pub const SEARCH_INTENT_MAX_BYTES: usize = 4096;
pub const SEARCH_LIMIT_DEFAULT: usize = 5;
pub const SEARCH_LIMIT_MAX: usize = 8;
pub const SEARCH_CANDIDATE_SUMMARY_MAX_BYTES: usize = 512;
pub const SEARCH_RESPONSE_MAX_BYTES: usize = 12 * 1024;
pub const DESCRIBE_RESPONSE_MAX_BYTES: usize = 16 * 1024;
pub const GATEWAY_INPUT_MAX_BYTES: usize = 64 * 1024;
pub const BACKEND_INPUT_MAX_BYTES: usize = 16 * 1024;
pub const BACKEND_RESULT_MAX_BYTES: usize = 16 * 1024;
pub const USAGE_PAGE_MAX_BYTES: usize = 8 * 1024;
pub const SEARCH_TEXT_MAX_BYTES: usize = 4096;
pub const REFERENCE_TTL_MILLIS: i64 = 10 * 60 * 1000;
pub const REFERENCE_MAX_PER_RUN: usize = 64;
pub const RULE_STRENGTH: f64 = 0.25;
pub const RULE_CORRECTION_CLAMP: f64 = 0.5;
pub const RRF_K: f64 = 60.0;
pub const RERANK_TOP: usize = 30;
pub const RERANK_PREFERRED_MAX: usize = 8;
pub const RERANK_TARGET_MAX: usize = RERANK_TOP + RERANK_PREFERRED_MAX;
pub const EXTRACT_MAX_FEEDBACK: usize = 4;
pub const EXTRACT_INPUT_MAX_BYTES: usize = 16 * 1024;
pub const EXTRACT_USER_MESSAGE_MAX_BYTES: usize = 8 * 1024;
pub const EXTRACT_RECENT_DECISIONS: usize = 8;
pub const EXTRACT_OUTPUT_MAX_BYTES: usize = 4096;
pub const EXTRACT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);
pub const WORKER_REQUEST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);
pub const WORKER_LOAD_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);
pub const WORKER_LINE_MAX_BYTES: usize = 2 * 1024 * 1024;
pub const WORKER_STDERR_MAX_BYTES: usize = 64 * 1024;
pub const WORKER_PENDING_MAX: usize = 8;

/// Modes fixed by the guide. `Direct` keeps the existing M2A behaviour; the new discovery path is
/// opt-in and disabled by default.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SelectionMode {
    Disabled,
    Direct,
    Discovery,
}

impl SelectionMode {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "disabled" => Some(Self::Disabled),
            "direct" => Some(Self::Direct),
            "discovery" => Some(Self::Discovery),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Disabled => "disabled",
            Self::Direct => "direct",
            Self::Discovery => "discovery",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExtractionSetting {
    ConfiguredConversationProvider,
}

/// Parsed `SAAA_TOOL_SELECTION_CONFIG`. An unset file means the legacy direct mode. A malformed
/// file disables discovery and keeps a fixed diagnostic. Discovery never silently falls back to
/// offering every catalog entry.
#[derive(Clone, Debug)]
pub struct ToolSelectionConfig {
    pub mode: SelectionMode,
    pub python_path: Option<PathBuf>,
    pub model_manifest_path: Option<PathBuf>,
    pub extraction: ExtractionSetting,
    pub diagnostic: Option<&'static str>,
}

impl ToolSelectionConfig {
    pub fn direct() -> Self {
        Self {
            mode: SelectionMode::Direct,
            python_path: None,
            model_manifest_path: None,
            extraction: ExtractionSetting::ConfiguredConversationProvider,
            diagnostic: None,
        }
    }

    pub fn disabled(diagnostic: &'static str) -> Self {
        Self {
            mode: SelectionMode::Disabled,
            python_path: None,
            model_manifest_path: None,
            extraction: ExtractionSetting::ConfiguredConversationProvider,
            diagnostic: Some(diagnostic),
        }
    }

    pub fn from_environment() -> Self {
        let path = std::env::var_os(CONFIG_ENV).map(PathBuf::from);
        Self::from_path(path.as_deref())
    }

    pub fn from_path(path: Option<&Path>) -> Self {
        let Some(path) = path else {
            return Self::direct();
        };
        match std::fs::metadata(path) {
            Ok(metadata) if metadata.len() <= CONFIG_MAX_BYTES => {}
            Ok(_) => return Self::disabled("tool-selection configuration is too large"),
            Err(_) => return Self::disabled("tool-selection configuration is unreadable"),
        }
        let raw = match std::fs::read(path) {
            Ok(raw) if raw.len() as u64 <= CONFIG_MAX_BYTES => raw,
            _ => return Self::disabled("tool-selection configuration is unreadable"),
        };
        let parsed: ConfigDocument = match serde_json::from_slice(&raw) {
            Ok(parsed) => parsed,
            Err(_) => return Self::disabled("tool-selection configuration is invalid"),
        };
        parsed.validate()
    }

    pub fn discovery_enabled(&self) -> bool {
        self.mode == SelectionMode::Discovery
    }

    pub fn is_valid_discovery(&self) -> bool {
        self.mode != SelectionMode::Discovery
            || (self.python_path.is_some() && self.model_manifest_path.is_some())
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ConfigDocument {
    format_version: u32,
    mode: String,
    python_path: Option<String>,
    model_manifest_path: Option<String>,
    extraction: Option<String>,
}

impl ConfigDocument {
    fn validate(self) -> ToolSelectionConfig {
        if self.format_version != CONFIG_FORMAT_VERSION {
            return ToolSelectionConfig::disabled(
                "tool-selection configuration version is unknown",
            );
        }
        let Some(mode) = SelectionMode::parse(&self.mode) else {
            return ToolSelectionConfig::disabled("tool-selection mode is unknown");
        };
        if let Some(extraction) = self.extraction.as_deref() {
            if extraction != "configured-conversation-provider" {
                return ToolSelectionConfig::disabled("tool-selection extraction is unknown");
            }
        }
        let python_path = self.python_path.map(PathBuf::from);
        let model_manifest_path = self.model_manifest_path.map(PathBuf::from);
        if mode == SelectionMode::Discovery
            && (python_path.is_none() || model_manifest_path.is_none())
        {
            return ToolSelectionConfig::disabled("tool-selection discovery paths are missing");
        }
        ToolSelectionConfig {
            mode,
            python_path,
            model_manifest_path,
            extraction: ExtractionSetting::ConfiguredConversationProvider,
            diagnostic: None,
        }
    }
}

/// Host-confirmed identity and scope for one selection. The model never supplies these values.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RequestContext {
    pub principal_id: String,
    pub conversation_id: String,
    pub run_id: Option<String>,
    pub input_message_id: Option<String>,
    pub project_id: Option<String>,
    pub task_id: Option<String>,
}

impl RequestContext {
    pub fn new(principal_id: &str, conversation_id: &str) -> Self {
        Self {
            principal_id: principal_id.to_string(),
            conversation_id: conversation_id.to_string(),
            run_id: None,
            input_message_id: None,
            project_id: None,
            task_id: None,
        }
    }

    pub fn with_run(mut self, run_id: Option<String>) -> Self {
        self.run_id = run_id;
        self
    }

    pub fn with_message(mut self, message_id: Option<String>) -> Self {
        self.input_message_id = message_id;
        self
    }

    pub fn with_project(mut self, project_id: Option<String>) -> Self {
        self.project_id = project_id;
        self
    }

    pub fn with_task(mut self, task_id: Option<String>) -> Self {
        self.task_id = task_id;
        self
    }

    pub fn scope_key(&self) -> String {
        self.run_id
            .clone()
            .unwrap_or_else(|| format!("conversation:{}", self.conversation_id))
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ToolSelectionErrorCode {
    InvalidInput,
    NotFound,
    NotAuthorized,
    StaleReference,
    SelectionChanged,
    Capacity,
    Unavailable,
    Timeout,
    Cancelled,
    Integrity,
    Storage,
}

impl ToolSelectionErrorCode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::InvalidInput => "invalid-input",
            Self::NotFound => "not-found",
            Self::NotAuthorized => "not-authorized",
            Self::StaleReference => "stale-reference",
            Self::SelectionChanged => "selection-changed",
            Self::Capacity => "capacity",
            Self::Unavailable => "unavailable",
            Self::Timeout => "timeout",
            Self::Cancelled => "cancelled",
            Self::Integrity => "integrity",
            Self::Storage => "storage",
        }
    }
}

/// Errors surfaced to the model. `message` is always a fixed safe sentence; internal details are
/// never carried here.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ToolSelectionError {
    pub code: ToolSelectionErrorCode,
    pub message: &'static str,
    pub retryable: bool,
}

impl ToolSelectionError {
    pub fn new(code: ToolSelectionErrorCode, message: &'static str) -> Self {
        Self {
            code,
            message,
            retryable: false,
        }
    }

    pub fn retryable(code: ToolSelectionErrorCode, message: &'static str) -> Self {
        Self {
            code,
            message,
            retryable: true,
        }
    }

    pub fn invalid() -> Self {
        Self::new(
            ToolSelectionErrorCode::InvalidInput,
            "The tool selection request is invalid.",
        )
    }

    pub fn storage() -> Self {
        Self::new(
            ToolSelectionErrorCode::Storage,
            "The tool selection store is unavailable.",
        )
    }

    pub fn unavailable() -> Self {
        Self::new(
            ToolSelectionErrorCode::Unavailable,
            "The tool selection service is unavailable.",
        )
    }

    pub fn not_found() -> Self {
        Self::new(
            ToolSelectionErrorCode::NotFound,
            "The reference is not available.",
        )
    }

    pub fn unauthorized() -> Self {
        Self::new(
            ToolSelectionErrorCode::NotAuthorized,
            "The reference is not available.",
        )
    }

    pub fn stale() -> Self {
        Self::new(
            ToolSelectionErrorCode::StaleReference,
            "The reference is stale. Search again.",
        )
    }

    pub fn changed() -> Self {
        Self::retryable(
            ToolSelectionErrorCode::SelectionChanged,
            "Tool selection changed. Search again.",
        )
    }

    pub fn capacity() -> Self {
        Self::new(
            ToolSelectionErrorCode::Capacity,
            "Too many references are open for this run.",
        )
    }
}

impl fmt::Display for ToolSelectionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.message)
    }
}

impl std::error::Error for ToolSelectionError {}

pub type ToolSelectionResult<T> = Result<T, ToolSelectionError>;

/// Current time as UTC unix milliseconds, the only time representation in the ledger.
pub fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

// ---------------------------------------------------------------------------------------------
// Semantic labels. Unknown values normalize to `Unknown`; they never widen a rule's scope.
// ---------------------------------------------------------------------------------------------

macro_rules! semantic_label {
    ($name:ident { $($variant:ident => $text:literal),+ $(,)? }) => {
        #[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
        #[serde(rename_all = "snake_case")]
        pub enum $name {
            $($variant,)+
            #[serde(other)]
            Unknown,
        }

        impl $name {
            pub fn parse(value: &str) -> Self {
                match value {
                    $($text => Self::$variant,)+
                    _ => Self::Unknown,
                }
            }

            pub fn as_str(self) -> &'static str {
                match self {
                    $(Self::$variant => $text,)+
                    Self::Unknown => "unknown",
                }
            }

            pub fn is_known(self) -> bool {
                !matches!(self, Self::Unknown)
            }
        }
    };
}

semantic_label!(Operation {
    Search => "search",
    Read => "read",
    Extract => "extract",
    Summarize => "summarize",
    Compare => "compare",
    Create => "create",
    Update => "update",
    Delete => "delete",
    Send => "send",
    Execute => "execute",
});

semantic_label!(ObjectType {
    DecisionRecord => "decision_record",
    CurrentInformation => "current_information",
    Document => "document",
    Table => "table",
    Code => "code",
    Message => "message",
    CalendarItem => "calendar_item",
    File => "file",
});

semantic_label!(Phase {
    Discover => "discover",
    Inspect => "inspect",
    Transform => "transform",
    Commit => "commit",
});

semantic_label!(InputKind {
    Text => "text",
    File => "file",
    Url => "url",
    Structured => "structured",
    None => "none",
});

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FeedbackKind {
    ToolChoice,
    Arguments,
    OutputQuality,
    SourceScope,
    TemporaryConstraint,
    ExplicitPositive,
    Revoke,
    Ambiguous,
    Unknown,
}

impl FeedbackKind {
    pub fn parse(value: &str) -> Self {
        match value {
            "tool_choice" => Self::ToolChoice,
            "arguments" => Self::Arguments,
            "output_quality" => Self::OutputQuality,
            "source_scope" => Self::SourceScope,
            "temporary_constraint" => Self::TemporaryConstraint,
            "explicit_positive" => Self::ExplicitPositive,
            "revoke" => Self::Revoke,
            "ambiguous" => Self::Ambiguous,
            _ => Self::Unknown,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::ToolChoice => "tool_choice",
            Self::Arguments => "arguments",
            Self::OutputQuality => "output_quality",
            Self::SourceScope => "source_scope",
            Self::TemporaryConstraint => "temporary_constraint",
            Self::ExplicitPositive => "explicit_positive",
            Self::Revoke => "revoke",
            Self::Ambiguous => "ambiguous",
            Self::Unknown => "unknown",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ScopeKind {
    User,
    Project,
    Task,
    Conversation,
}

impl ScopeKind {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "user" => Some(Self::User),
            "project" => Some(Self::Project),
            "task" => Some(Self::Task),
            "conversation" => Some(Self::Conversation),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Project => "project",
            Self::Task => "task",
            Self::Conversation => "conversation",
        }
    }

    /// Narrower scope wins. task > conversation > project > user, per the guide.
    pub fn priority(self) -> u8 {
        match self {
            Self::Task => 4,
            Self::Conversation => 3,
            Self::Project => 2,
            Self::User => 1,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Duration {
    Once,
    Persistent,
    Unspecified,
}

impl Duration {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "once" => Some(Self::Once),
            "persistent" => Some(Self::Persistent),
            "unspecified" => Some(Self::Unspecified),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Once => "once",
            Self::Persistent => "persistent",
            Self::Unspecified => "unspecified",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RuleAction {
    Avoid,
    Prefer,
    Pairwise,
    Forbid,
}

impl RuleAction {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Avoid => "avoid",
            Self::Prefer => "prefer",
            Self::Pairwise => "pairwise",
            Self::Forbid => "forbid",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Scenario {
    pub intent: String,
    pub operation: Operation,
    pub object_type: ObjectType,
    pub phase: Phase,
    pub input_kind: InputKind,
}

impl Scenario {
    pub fn degraded(intent: &str) -> Self {
        Self {
            intent: intent.to_string(),
            operation: Operation::Unknown,
            object_type: ObjectType::Unknown,
            phase: Phase::Unknown,
            input_kind: InputKind::Unknown,
        }
    }

    /// Stable semantic signature used in logs and tests. Intent is intentionally excluded from
    /// rule matching but included here because it identifies the decision scenario.
    pub fn canonical(&self) -> String {
        format!(
            "{}|{}|{}|{}|{}",
            self.intent,
            self.operation.as_str(),
            self.object_type.as_str(),
            self.phase.as_str(),
            self.input_kind.as_str()
        )
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FeedbackCondition {
    pub operation: Option<Operation>,
    pub object_type: Option<ObjectType>,
    pub phase: Option<Phase>,
    pub input_kind: Option<InputKind>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Evidence {
    pub start: usize,
    pub end: usize,
    pub text: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ExtractedFeedback {
    pub kind: FeedbackKind,
    pub decision_id: Option<String>,
    pub rejected_tool_id: Option<String>,
    pub preferred_tool_id: Option<String>,
    pub scope: ScopeKind,
    pub duration: Duration,
    pub evidence: Evidence,
    pub condition: FeedbackCondition,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ExtractionOutput {
    pub scenario: Scenario,
    pub feedback: Vec<ExtractedFeedback>,
}

/// One ranked candidate stored with a decision.
#[derive(Clone, Debug, PartialEq)]
pub struct CandidateRecord {
    pub revision_id: String,
    pub tool_id: String,
    pub lex_rank: Option<i64>,
    pub vec_rank: Option<i64>,
    pub raw_score: Option<f64>,
    pub base_score: f64,
    pub final_score: f64,
    pub rule_ids: Vec<String>,
    pub final_rank: i64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct DecisionRecord {
    pub id: String,
    pub principal_id: String,
    pub conversation_id: String,
    pub run_id: Option<String>,
    pub message_id: Option<String>,
    pub scenario: Scenario,
    pub catalog_epoch: i64,
    pub acl_epoch: i64,
    pub rule_epoch: i64,
    pub model_hash: Option<String>,
    pub status: DecisionStatus,
    pub created_at: i64,
    pub candidates: Vec<CandidateRecord>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DecisionStatus {
    Ok,
    NoMatch,
    Degraded,
}

impl DecisionStatus {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "ok" => Some(Self::Ok),
            "no_match" => Some(Self::NoMatch),
            "degraded" => Some(Self::Degraded),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::NoMatch => "no_match",
            Self::Degraded => "degraded",
        }
    }
}

/// Active rule loaded from the store. `active_rule` owns the matched condition/action.
#[derive(Clone, Debug, PartialEq)]
pub struct StoredRule {
    pub id: String,
    pub principal_id: String,
    pub scope_kind: ScopeKind,
    pub scope_id: String,
    pub operation: String,
    pub object_type: String,
    pub phase: Option<String>,
    pub input_kind: Option<String>,
    pub source_constraint: Option<String>,
    pub target_tool_id: Option<String>,
    pub target_revision_id: Option<String>,
    pub preferred_tool_id: Option<String>,
    pub action: RuleAction,
    pub strength: f64,
    pub state: RuleState,
    pub created_at: i64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RuleState {
    Active,
    Revoked,
    Superseded,
}

impl RuleState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Revoked => "revoked",
            Self::Superseded => "superseded",
        }
    }
}

/// Result of applying active rules to a reranked candidate order.
#[derive(Clone, Debug, PartialEq)]
pub struct CorrectedCandidate {
    pub revision_id: String,
    pub tool_id: String,
    pub base_score: f64,
    pub correction: f64,
    pub final_score: f64,
    pub rule_ids: Vec<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CorrectionOutcome {
    pub ordered: Vec<CorrectedCandidate>,
    pub ambiguous: bool,
}

impl Scenario {
    pub fn operation_known(&self) -> bool {
        self.operation.is_known()
    }

    pub fn object_known(&self) -> bool {
        self.object_type.is_known()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unset_configuration_keeps_direct_mode() {
        let config = ToolSelectionConfig::from_path(None);
        assert_eq!(config.mode, SelectionMode::Direct);
        assert!(!config.discovery_enabled());
    }

    #[test]
    fn invalid_configuration_disables_discovery_instead_of_offering_everything() {
        let directory = tempfile::tempdir().expect("tempdir");
        let path = directory.path().join("config.json");
        std::fs::write(&path, b"{\"formatVersion\":9,\"mode\":\"discovery\"}").expect("write");
        let config = ToolSelectionConfig::from_path(Some(&path));
        assert_eq!(config.mode, SelectionMode::Disabled);
        assert!(config.diagnostic.is_some());
    }

    #[test]
    fn discovery_requires_both_paths() {
        let directory = tempfile::tempdir().expect("tempdir");
        let path = directory.path().join("config.json");
        std::fs::write(
            &path,
            br#"{"formatVersion":1,"mode":"discovery","pythonPath":"/venv/bin/python"}"#,
        )
        .expect("write");
        let config = ToolSelectionConfig::from_path(Some(&path));
        assert_eq!(config.mode, SelectionMode::Disabled);
    }

    #[test]
    fn unknown_semantic_labels_normalize_to_unknown() {
        assert_eq!(Operation::parse("not-a-real-op"), Operation::Unknown);
        assert_eq!(ObjectType::parse("nope"), ObjectType::Unknown);
        assert!(!Operation::parse("nope").is_known());
    }

    #[test]
    fn task_scope_is_narrower_than_user_scope() {
        assert!(ScopeKind::Task.priority() > ScopeKind::Conversation.priority());
        assert!(ScopeKind::Conversation.priority() > ScopeKind::Project.priority());
        assert!(ScopeKind::Project.priority() > ScopeKind::User.priority());
    }
}
