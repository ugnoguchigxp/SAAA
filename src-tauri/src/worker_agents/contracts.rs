//! Frozen contracts of the worker-agent domain (docs/plans/worker-agents.md §4).
//!
//! Everything the workstreams share lives here: persisted-profile types, the delegate/outcome
//! envelope, the discovery offer, and the three traits that decouple the generic executor from
//! the model transport, the tool host and the per-profile attempt logic.
use crate::RunCancellation;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::time::Duration;
use ts_rs::TS;

pub(crate) const WORKER_LANE: &str = "worker";
pub(crate) const WORKER_JOB_KIND: &str = "worker_task";
pub(crate) const WEB_SEARCH_PROFILE_ID: &str = "web_search";
/// Builtin tool keys a profile may reference with `ToolRefKind::Builtin`.
pub(crate) const BUILTIN_TOOL_KEYS: [&str; 2] = ["web_search", "fetch_content"];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ReviewState {
    Draft,
    Approved,
    Rejected,
    Superseded,
}

impl ReviewState {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Draft => "draft",
            Self::Approved => "approved",
            Self::Rejected => "rejected",
            Self::Superseded => "superseded",
        }
    }
    pub(crate) fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "draft" => Self::Draft,
            "approved" => Self::Approved,
            "rejected" => Self::Rejected,
            "superseded" => Self::Superseded,
            _ => return None,
        })
    }
}

/// Closed set: a new output kind is a code change, never data.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub(crate) enum OutputKind {
    WebClaimsV1,
    JsonV1,
}

impl OutputKind {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::WebClaimsV1 => "web_claims_v1",
            Self::JsonV1 => "json_v1",
        }
    }
    pub(crate) fn parse(value: &str) -> Option<Self> {
        match value {
            "web_claims_v1" => Some(Self::WebClaimsV1),
            "json_v1" => Some(Self::JsonV1),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Tier {
    Local,
    LocalLarge,
    Cloud,
}

impl Tier {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Local => "local",
            Self::LocalLarge => "local_large",
            Self::Cloud => "cloud",
        }
    }
    pub(crate) fn parse(value: &str) -> Option<Self> {
        match value {
            "local" => Some(Self::Local),
            "local_large" => Some(Self::LocalLarge),
            "cloud" => Some(Self::Cloud),
            _ => None,
        }
    }
}

/// v1 has no automatic cloud spend.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub(crate) enum CloudPolicy {
    Never,
    RequireApproval,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct WorkerLimits {
    pub max_steps: u32,
    pub max_same_tier_retries: u32,
    #[ts(type = "number")]
    pub deadline_ms: u64,
    #[ts(type = "number")]
    pub sync_wait_ms: u64,
    pub max_restarts: u32,
}

impl Default for WorkerLimits {
    fn default() -> Self {
        Self {
            max_steps: 6,
            max_same_tier_retries: 1,
            deadline_ms: 40_000,
            sync_wait_ms: 15_000,
            max_restarts: 1,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct TierPolicy {
    pub max_tier: Tier,
    pub cloud: CloudPolicy,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ToolRefKind {
    Builtin,
    Catalog,
}

/// Toolchain reference. A catalog ref is pinned to one revision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ToolRef {
    pub kind: ToolRefKind,
    pub key: String,
    pub catalog_revision_id: Option<String>,
}

/// Host-verifiable predicates only.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct CompletionCriteria {
    pub min_items: u32,
    pub sources_must_be_host_recorded: bool,
}

/// IPC upsert input for one profile revision.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ProfileDraft {
    pub profile_id: String,
    pub purpose: String,
    pub system_context: String,
    pub skill_revision_ids: Vec<String>,
    pub tools: Vec<ToolRef>,
    #[ts(type = "unknown")]
    pub input_schema: serde_json::Value,
    pub output_kind: OutputKind,
    #[ts(type = "unknown | null")]
    pub output_schema: Option<serde_json::Value>,
    pub completion: CompletionCriteria,
    pub limits: WorkerLimits,
    pub tier_policy: TierPolicy,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct SkillDraft {
    pub skill_id: Option<String>,
    pub name: String,
    pub body: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SkillSaved {
    pub skill_id: String,
    pub revision_id: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub(crate) enum WebSearchMode {
    Inline,
    Worker,
}

impl WebSearchMode {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Inline => "inline",
            Self::Worker => "worker",
        }
    }
    pub(crate) fn parse(value: &str) -> Option<Self> {
        match value {
            "inline" => Some(Self::Inline),
            "worker" => Some(Self::Worker),
            _ => None,
        }
    }
}

// ---- IPC read models (§4.3) -------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RevisionSummary {
    pub revision_id: String,
    pub revision: u32,
    pub review_state: ReviewState,
    pub purpose: String,
    pub definition_hash: String,
    pub tool_keys: Vec<String>,
    pub output_kind: OutputKind,
    #[ts(type = "number")]
    pub created_at_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WorkerAgentSummary {
    pub profile_id: String,
    pub origin: String,
    pub enabled: bool,
    pub pinned_offer: bool,
    pub current: Option<RevisionSummary>,
    pub draft: Option<RevisionSummary>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WorkerAgentDetail {
    pub profile_id: String,
    pub origin: String,
    pub enabled: bool,
    pub pinned_offer: bool,
    pub revisions: Vec<RevisionSummary>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WorkerTaskSummary {
    pub task_id: String,
    pub profile_id: String,
    pub state: String,
    pub delivery: String,
    pub failure_code: Option<String>,
    #[ts(type = "number")]
    pub created_at_ms: i64,
    #[ts(type = "number")]
    pub updated_at_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub(crate) struct BlocklistEntry {
    pub url_hash: String,
    pub host: String,
    pub reason: String,
    #[ts(type = "number")]
    pub created_at_ms: i64,
}

// ---- Delegation envelope (§4.1) ----------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct DelegateRequest {
    pub agent: String,
    pub input: serde_json::Value,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum FailureCode {
    NoMatchingAgent,
    StaleOffer,
    InputInvalid,
    Duplicate,
    NoSafeSources,
    NoResults,
    BudgetExhausted,
    DeadlineExceeded,
    ToolUnavailable,
    InvalidOutput,
    CompletionUnmet,
    EscalationRequiresApproval,
    EscalationDeclined,
    OutcomeUnknown,
    Cancelled,
    Interrupted,
}

impl FailureCode {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::NoMatchingAgent => "no_matching_agent",
            Self::StaleOffer => "stale_offer",
            Self::InputInvalid => "input_invalid",
            Self::Duplicate => "duplicate",
            Self::NoSafeSources => "no_safe_sources",
            Self::NoResults => "no_results",
            Self::BudgetExhausted => "budget_exhausted",
            Self::DeadlineExceeded => "deadline_exceeded",
            Self::ToolUnavailable => "tool_unavailable",
            Self::InvalidOutput => "invalid_output",
            Self::CompletionUnmet => "completion_unmet",
            Self::EscalationRequiresApproval => "escalation_requires_approval",
            Self::EscalationDeclined => "escalation_declined",
            Self::OutcomeUnknown => "outcome_unknown",
            Self::Cancelled => "cancelled",
            Self::Interrupted => "interrupted",
        }
    }
    pub(crate) fn parse(value: &str) -> Option<Self> {
        serde_json::from_value(serde_json::Value::String(value.to_string())).ok()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WorkerFailure {
    pub code: FailureCode,
    pub retryable: bool,
}

/// The only shape the conversation side receives.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub(crate) enum WorkerOutcome {
    Pending {
        task_id: String,
    },
    Succeeded {
        task_id: String,
        output: WorkerOutput,
    },
    Failed {
        task_id: Option<String>,
        failure: WorkerFailure,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum WorkerOutput {
    WebClaimsV1(WebClaims),
    JsonV1(serde_json::Value),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct WebClaims {
    pub claims: Vec<WebClaim>,
    pub excluded: ExcludedSummary,
    /// Computed by the host, never self-reported by the worker.
    pub confidence: Confidence,
    pub coverage: Coverage,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct WebClaim {
    pub text: String,
    pub source_url: String,
    pub basis: ClaimBasis,
    pub published_or_fetched_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ExcludedSummary {
    pub count: u32,
    pub categories: Vec<String>,
    pub domains: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Confidence {
    Corroborated,
    SingleSource,
    SnippetOnly,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Coverage {
    Complete,
    Partial,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ClaimBasis {
    Page,
    Snippet,
}

// ---- Discovery (§5.1) --------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum OfferStatus {
    Ok,
    Ambiguous,
    NoMatch,
    Degraded,
}

impl OfferStatus {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Ambiguous => "ambiguous",
            Self::NoMatch => "no_match",
            Self::Degraded => "degraded",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct OfferCandidate {
    pub profile_id: String,
    pub revision_id: String,
    /// Truncated to 200 characters before it reaches a prompt.
    pub purpose: String,
    pub input_schema: serde_json::Value,
    pub score: f64,
    pub pinned: bool,
}

/// What the conversation agent is shown. Ranking is not authority: a `delegate` is accepted only
/// for a candidate persisted under `decision_id` with the same epochs.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Offer {
    pub decision_id: String,
    pub status: OfferStatus,
    pub registry_epoch: i64,
    pub acl_epoch: i64,
    pub candidates: Vec<OfferCandidate>,
}

// ---- Admission (§5.2) --------------------------------------------------------------------

pub(crate) struct AdmitRequest<'a> {
    pub conversation_id: &'a str,
    pub input_message_id: &'a str,
    pub origin_job_key: &'a str,
    pub decision_id: &'a str,
    pub delegate: &'a DelegateRequest,
    /// Remaining deadline of the conversation job, used to cap the synchronous wait.
    pub conversation_deadline_ms: i64,
}

// ---- Loaded (immutable) profile revision --------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ToolEffect {
    Pure,
    Read,
    Write,
    Unknown,
}

impl ToolEffect {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Pure => "pure",
            Self::Read => "read",
            Self::Write => "write",
            Self::Unknown => "unknown",
        }
    }
    pub(crate) fn parse(value: &str) -> Option<Self> {
        match value {
            "pure" => Some(Self::Pure),
            "read" => Some(Self::Read),
            "write" => Some(Self::Write),
            "unknown" => Some(Self::Unknown),
            _ => None,
        }
    }
    /// Only these effects may be retried or re-queued without replaying an external effect.
    pub(crate) fn is_read_only(self) -> bool {
        matches!(self, Self::Pure | Self::Read)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LoadedTool {
    pub kind: ToolRefKind,
    pub key: String,
    pub catalog_revision_id: Option<String>,
    pub effect: ToolEffect,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LoadedSkill {
    pub name: String,
    pub body: String,
}

/// One approved, immutable revision with everything an attempt needs. Tasks pin a revision id
/// at admission; later edits never affect an in-flight task.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct LoadedRevision {
    pub profile_id: String,
    pub revision_id: String,
    pub revision: u32,
    pub review_state: ReviewState,
    pub purpose: String,
    pub system_context: String,
    pub skills: Vec<LoadedSkill>,
    pub tools: Vec<LoadedTool>,
    pub input_schema: serde_json::Value,
    pub output_kind: OutputKind,
    pub output_schema: Option<serde_json::Value>,
    pub completion: CompletionCriteria,
    pub limits: WorkerLimits,
    pub tier_policy: TierPolicy,
    pub definition_hash: String,
}

impl LoadedRevision {
    pub(crate) fn all_tools_read_only(&self) -> bool {
        self.tools.iter().all(|tool| tool.effect.is_read_only())
    }
}

// ---- Executor seams ------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RouteLocation {
    Local,
    Cloud,
}

/// Opaque handle for one rung of the model ladder. The transport that produced it maps
/// `fingerprint` back to the settings snapshot fixed at job start.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TierRoute {
    pub tier: Tier,
    pub fingerprint: String,
    pub location: RouteLocation,
}

/// Model transport. Implemented over the conversation direct route in the runtime layer; fakes in
/// tests. Calls take the foreground inference slot: the conversation agent holds the shared read
/// lock while it waits for a worker, so a background (write-lock) slot would deadlock.
#[async_trait]
pub(crate) trait WorkerModel: Send + Sync {
    /// Ordered rungs the task may climb. Cheapest first.
    fn tier_routes(&self) -> Result<Vec<TierRoute>, String>;
    async fn complete(
        &self,
        route: &TierRoute,
        system: &str,
        input: &str,
        cancellation: &RunCancellation,
        timeout: Duration,
    ) -> Result<String, String>;
}

/// Tool host. Returns the raw JSON envelope the existing agent-tool dispatcher produces.
/// Ledger bookkeeping (reserve, dispatched, settled) is done by a decorator in the executor, so
/// attempt logic can simply call it.
#[async_trait]
pub(crate) trait ToolRunner: Send + Sync {
    async fn run(
        &self,
        tool_key: &str,
        arguments_json: &str,
        timeout: Duration,
        cancellation: &RunCancellation,
    ) -> String;
}

pub(crate) struct AttemptEnv<'a> {
    pub task_id: &'a str,
    pub attempt_ordinal: u32,
    pub revision: &'a LoadedRevision,
    /// Delegated input, already validated against the revision's input schema.
    pub input: &'a serde_json::Value,
    /// The persisted user utterance that triggered the delegation (for `user_supplied` URLs).
    pub user_text: &'a str,
    pub writer: &'a crate::persistence::SqliteWriter,
    pub model: &'a dyn WorkerModel,
    pub route: &'a TierRoute,
    pub tools: &'a dyn ToolRunner,
    pub cancellation: &'a RunCancellation,
    pub deadline: tokio::time::Instant,
}

/// Why an attempt ended without an output. The executor decides retry/escalate/terminal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum AttemptError {
    /// Malformed model output: same-tier retry, then escalate.
    InvalidOutput(String),
    /// Transport or timeout failure: same-tier retry, then escalate.
    Transport(String),
    /// Output validated but the host-checked completion criteria were not met.
    CompletionUnmet,
    /// A terminal failure that no higher tier can fix (no safe sources, no results, ...).
    Terminal(FailureCode),
    Cancelled,
    DeadlineExceeded,
}

/// Per-output-kind attempt logic (the web-search loop, a generic JSON worker, ...).
#[async_trait]
pub(crate) trait AttemptRunner: Send + Sync {
    async fn run(&self, env: &AttemptEnv<'_>) -> Result<WorkerOutput, AttemptError>;
}

// ---- TypeScript bindings -------------------------------------------------------------------

pub(crate) fn typescript_bindings() -> String {
    [
        ReviewState::decl(&Default::default()),
        OutputKind::decl(&Default::default()),
        Tier::decl(&Default::default()),
        CloudPolicy::decl(&Default::default()),
        WorkerLimits::decl(&Default::default()),
        TierPolicy::decl(&Default::default()),
        ToolRefKind::decl(&Default::default()),
        ToolRef::decl(&Default::default()),
        CompletionCriteria::decl(&Default::default()),
        ProfileDraft::decl(&Default::default()),
        SkillDraft::decl(&Default::default()),
        SkillSaved::decl(&Default::default()),
        WebSearchMode::decl(&Default::default()),
        RevisionSummary::decl(&Default::default()),
        WorkerAgentSummary::decl(&Default::default()),
        WorkerAgentDetail::decl(&Default::default()),
        WorkerTaskSummary::decl(&Default::default()),
        BlocklistEntry::decl(&Default::default()),
    ]
    .map(|decl| format!("export {decl}"))
    .join("\n")
}

pub fn typescript_file() -> String {
    format!(
        "// Generated from src-tauri/src/worker_agents/contracts.rs. Do not edit.\n{}\n",
        typescript_bindings()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failure_codes_round_trip_through_their_names() {
        for code in [
            FailureCode::NoSafeSources,
            FailureCode::EscalationRequiresApproval,
            FailureCode::OutcomeUnknown,
        ] {
            assert_eq!(FailureCode::parse(code.as_str()), Some(code));
        }
        assert_eq!(FailureCode::parse("nope"), None);
    }

    #[test]
    fn only_pure_and_read_effects_are_retry_safe() {
        assert!(ToolEffect::Pure.is_read_only());
        assert!(ToolEffect::Read.is_read_only());
        assert!(!ToolEffect::Write.is_read_only());
        assert!(!ToolEffect::Unknown.is_read_only());
    }

    #[test]
    fn tiers_are_ordered_cheapest_first() {
        assert!(Tier::Local < Tier::LocalLarge && Tier::LocalLarge < Tier::Cloud);
    }

    #[test]
    fn typescript_bindings_cover_the_ipc_types() {
        let text = typescript_bindings();
        for name in [
            "ProfileDraft",
            "WorkerAgentSummary",
            "BlocklistEntry",
            "WebSearchMode",
        ] {
            assert!(text.contains(&format!("type {name}")), "{name} missing");
        }
    }
}
