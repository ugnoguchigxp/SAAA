//! Configuration, request context, error envelope, semantic labels and limits for the
//! tool-selection D0–D3 milestone.
//!
//! Everything here is pure data plus parsing. No SQL and no inference live in this module so the
//! same types can be used by tests, the gateway and the L-Lang backend adapter.

use serde::{Deserialize, Serialize};
use std::fmt;
use std::path::{Path, PathBuf};
#[path = "contracts/scope_kind.rs"]
mod scope_kind;
#[path = "contracts/selection_mode.rs"]
pub(super) mod selection_mode;
pub use scope_kind::{
    CandidateRecord, CorrectedCandidate, CorrectionOutcome, DecisionRecord, DecisionStatus,
    Duration, Evidence, ExtractedFeedback, FeedbackCondition, RuleAction, RuleState, Scenario,
    ScopeKind, StoredRule,
};
#[allow(unused_imports)]
use selection_mode::restrict_mock_to_fixture_environment;
pub use selection_mode::{
    now_ms, FeedbackKind, InputKind, ObjectType, Operation, Phase, ToolSelectionError,
    ToolSelectionErrorCode, ToolSelectionResult,
};
pub use selection_mode::{
    ExtractionSetting, RequestContext, SelectionMode, ToolSelectionConfig, EXTRACT_INPUT_MAX_BYTES,
    EXTRACT_MAX_FEEDBACK, EXTRACT_OUTPUT_MAX_BYTES, EXTRACT_PROMPT_TOOLS_MAX,
    EXTRACT_RECENT_DECISIONS, EXTRACT_TIMEOUT, EXTRACT_USER_MESSAGE_MAX_BYTES,
    RERANK_PREFERRED_MAX, RERANK_TARGET_MAX, RERANK_TOP, WORKER_LINE_MAX_BYTES,
    WORKER_LOAD_TIMEOUT, WORKER_PENDING_MAX, WORKER_REQUEST_TIMEOUT, WORKER_SPAWN_FAILURE_LIMIT,
    WORKER_STDERR_MAX_BYTES,
};
pub use selection_mode::{
    BACKEND_INPUT_MAX_BYTES, BACKEND_RESULT_MAX_BYTES, CONFIG_ENV, CONFIG_FORMAT_VERSION,
    CONFIG_MAX_BYTES, DESCRIBE_RESPONSE_MAX_BYTES, GATEWAY_INPUT_MAX_BYTES, MOCK_FIXTURE_ENV,
    REFERENCE_MAX_PER_RUN, REFERENCE_TTL_MILLIS, RRF_K, RULE_CORRECTION_CLAMP, RULE_STRENGTH,
    SEARCH_CANDIDATE_SUMMARY_MAX_BYTES, SEARCH_INTENT_MAX_BYTES, SEARCH_LIMIT_DEFAULT,
    SEARCH_LIMIT_MAX, SEARCH_RESPONSE_MAX_BYTES, SEARCH_TEXT_MAX_BYTES, USAGE_PAGE_MAX_BYTES,
};
