use super::*;
/// Environment variable that points at the tool-selection configuration file.
pub const CONFIG_ENV: &str = "SAAA_TOOL_SELECTION_CONFIG";
/// Deliberate opt-in required in addition to the mock configuration document. The launcher also
/// supplies a smoke marker and a separate absolute data directory, so fixture observations
/// cannot accidentally become part of the normal application ledger.
pub const MOCK_FIXTURE_ENV: &str = "SAAA_ADAPTIVE_FIXTURE";
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
pub const EXTRACT_PROMPT_TOOLS_MAX: usize = 40;
pub const EXTRACT_OUTPUT_MAX_BYTES: usize = 4096;
pub const EXTRACT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);
pub const WORKER_REQUEST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);
pub const WORKER_LOAD_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);
pub const WORKER_LINE_MAX_BYTES: usize = 2 * 1024 * 1024;
pub const WORKER_STDERR_MAX_BYTES: usize = 64 * 1024;
pub const WORKER_PENDING_MAX: usize = 8;
pub const WORKER_SPAWN_FAILURE_LIMIT: u32 = 3;
/// Modes fixed by the guide. `Direct` keeps the existing M2A behaviour; the new discovery path is
/// opt-in and disabled by default.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SelectionMode {
    Disabled,
    Direct,
    Discovery,
    /// Deterministic developer-only lane. It never loads or contacts a model/provider.
    Mock,
}
impl SelectionMode {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "disabled" => Some(Self::Disabled),
            "direct" => Some(Self::Direct),
            "discovery" => Some(Self::Discovery),
            "mock" => Some(Self::Mock),
            _ => None,
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
    /// Optional absolute path to the host-managed external MCP source document. Presence is
    /// independent of discovery mode: MCP sources can be registered in direct mode too.
    pub mcp_sources_path: Option<PathBuf>,
    pub extraction: ExtractionSetting,
    pub diagnostic: Option<&'static str>,
}
impl ToolSelectionConfig {
    pub fn direct() -> Self {
        Self {
            mode: SelectionMode::Direct,
            python_path: None,
            model_manifest_path: None,
            mcp_sources_path: None,
            extraction: ExtractionSetting::ConfiguredConversationProvider,
            diagnostic: None,
        }
    }

    pub fn disabled(diagnostic: &'static str) -> Self {
        Self {
            mode: SelectionMode::Disabled,
            python_path: None,
            model_manifest_path: None,
            mcp_sources_path: None,
            extraction: ExtractionSetting::ConfiguredConversationProvider,
            diagnostic: Some(diagnostic),
        }
    }

    pub fn from_environment() -> Self {
        let path = std::env::var_os(CONFIG_ENV).map(PathBuf::from);
        restrict_mock_to_fixture_environment(
            Self::from_path(path.as_deref()),
            std::env::var_os(MOCK_FIXTURE_ENV).as_deref(),
            std::env::var_os("SAAA_SMOKE_MARKER_ID").as_deref(),
            std::env::var_os("SAAA_SMOKE_DATA_DIR").as_deref(),
        )
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
pub(super) fn mock_fixture_environment_is_isolated(
    fixture_enabled: Option<&std::ffi::OsStr>,
    smoke_marker: Option<&std::ffi::OsStr>,
    smoke_data_dir: Option<&std::ffi::OsStr>,
) -> bool {
    fixture_enabled == Some(std::ffi::OsStr::new("1"))
        && smoke_marker.is_some_and(|value| !value.is_empty())
        && smoke_data_dir.is_some_and(|value| Path::new(value).is_absolute())
}
pub(super) fn restrict_mock_to_fixture_environment(
    config: ToolSelectionConfig,
    fixture_enabled: Option<&std::ffi::OsStr>,
    smoke_marker: Option<&std::ffi::OsStr>,
    smoke_data_dir: Option<&std::ffi::OsStr>,
) -> ToolSelectionConfig {
    if config.mode == SelectionMode::Mock
        && !mock_fixture_environment_is_isolated(fixture_enabled, smoke_marker, smoke_data_dir)
    {
        ToolSelectionConfig::disabled(
            "tool-selection mock mode requires the isolated adaptive fixture launcher",
        )
    } else {
        config
    }
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ConfigDocument {
    pub(super) format_version: u32,
    pub(super) mode: String,
    pub(super) python_path: Option<String>,
    pub(super) model_manifest_path: Option<String>,
    #[serde(default)]
    pub(super) mcp_sources_path: Option<String>,
    pub(super) extraction: Option<String>,
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
        if mode == SelectionMode::Mock && !cfg!(debug_assertions) {
            return ToolSelectionConfig::disabled(
                "tool-selection mock mode is available only in development builds",
            );
        }
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
        let mcp_sources_path = match self.mcp_sources_path {
            None => None,
            Some(path) if path.is_empty() => None,
            Some(path) => {
                let path = PathBuf::from(path);
                if !path.is_absolute() {
                    return ToolSelectionConfig::disabled(
                        "tool-selection mcp sources path must be absolute",
                    );
                }
                Some(path)
            }
        };
        ToolSelectionConfig {
            mode,
            python_path,
            model_manifest_path,
            mcp_sources_path,
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
}
);
semantic_label!(ObjectType {
    DecisionRecord => "decision_record",
    CurrentInformation => "current_information",
    Document => "document",
    Table => "table",
    Code => "code",
    Message => "message",
    CalendarItem => "calendar_item",
    File => "file",
}
);
semantic_label!(Phase {
    Discover => "discover",
    Inspect => "inspect",
    Transform => "transform",
    Commit => "commit",
}
);
semantic_label!(InputKind {
    Text => "text",
    File => "file",
    Url => "url",
    Structured => "structured",
    None => "none",
}
);
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
