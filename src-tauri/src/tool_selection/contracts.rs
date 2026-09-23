//! Configuration, request context, error envelope, semantic labels and limits for the
//! tool-selection D0–D3 milestone.
//!
//! Everything here is pure data plus parsing. No SQL and no inference live in this module so the
//! same types can be used by tests, the gateway and the L-Lang backend adapter.

use serde::{Deserialize, Serialize};
use std::fmt;
use std::path::{Path, PathBuf};
#[path = "contracts/selection_mode.rs"]
pub(super) mod selection_mode;
#[path = "contracts/scope_kind.rs"]
mod scope_kind;
pub use selection_mode::{CONFIG_ENV, MOCK_FIXTURE_ENV, CONFIG_FORMAT_VERSION, CONFIG_MAX_BYTES, SEARCH_INTENT_MAX_BYTES, SEARCH_LIMIT_DEFAULT, SEARCH_LIMIT_MAX, SEARCH_CANDIDATE_SUMMARY_MAX_BYTES, SEARCH_RESPONSE_MAX_BYTES, DESCRIBE_RESPONSE_MAX_BYTES, GATEWAY_INPUT_MAX_BYTES, BACKEND_INPUT_MAX_BYTES, BACKEND_RESULT_MAX_BYTES, USAGE_PAGE_MAX_BYTES, SEARCH_TEXT_MAX_BYTES, REFERENCE_TTL_MILLIS, REFERENCE_MAX_PER_RUN, RULE_STRENGTH, RULE_CORRECTION_CLAMP, RRF_K};
pub use selection_mode::{RERANK_TOP, RERANK_PREFERRED_MAX, RERANK_TARGET_MAX, EXTRACT_MAX_FEEDBACK, EXTRACT_INPUT_MAX_BYTES, EXTRACT_USER_MESSAGE_MAX_BYTES, EXTRACT_RECENT_DECISIONS, EXTRACT_PROMPT_TOOLS_MAX, EXTRACT_OUTPUT_MAX_BYTES, EXTRACT_TIMEOUT, WORKER_REQUEST_TIMEOUT, WORKER_LOAD_TIMEOUT, WORKER_LINE_MAX_BYTES, WORKER_STDERR_MAX_BYTES, WORKER_PENDING_MAX, WORKER_SPAWN_FAILURE_LIMIT, SelectionMode, ExtractionSetting, ToolSelectionConfig, RequestContext};
pub use selection_mode::{ToolSelectionErrorCode, ToolSelectionError, ToolSelectionResult, now_ms, FeedbackKind, Operation, ObjectType, Phase, InputKind};
use selection_mode::{restrict_mock_to_fixture_environment};
pub use scope_kind::{ScopeKind, Duration, RuleAction, Scenario, FeedbackCondition, Evidence, ExtractedFeedback, CandidateRecord, DecisionRecord, DecisionStatus, StoredRule, RuleState, CorrectedCandidate, CorrectionOutcome};
