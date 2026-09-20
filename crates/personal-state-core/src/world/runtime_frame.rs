//! M2A WorldFrame: the persisted WorldSlice plus the *current* state of trusted
//! runtimes (meeting / coding). Pure types, normalization, canonical digests and
//! bounded budget assembly. No IO, no clock, no owner mutation.
//!
//! A `WorldFrame` is a diagnostic snapshot valid at one instant. It is not a
//! persistence format and its runtime views are never written to the World
//! ledger. Current state is re-read by the adapter on every request (R6/R7).

use super::slice_v2::WorldSliceV2;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const WORLD_FRAME_SCHEMA_VERSION: i64 = 1;
pub const MAX_RUNTIME_REFS: usize = 8;
pub const MAX_INPUT_RUNTIME_REFS: usize = 1_000;
pub const MAX_FRAME_BYTES: usize = 8_192;
pub const MAX_RUNTIME_BYTES: usize = 2_048;
pub const MAX_GRAPH_BYTES: usize = 6_144;
pub const MAX_TOTAL_NODES: usize = 30;
pub const MAX_NOTICES: usize = 16;
pub const MAX_TTL_MS: u64 = 1_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeKind {
    MeetingSession,
    CodingJob,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeRef {
    pub kind: RuntimeKind,
    pub id: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimePhase {
    Starting,
    Running,
    Paused,
    Stopping,
    Terminal,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MeetingLivePhase {
    Idle,
    Preflight,
    Ready,
    Active,
    Paused,
    Stopping,
    Completed,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MeetingOwnerState {
    Active,
    Paused,
    Stopping,
    Completed,
    Saved,
    Failed,
    Interrupted,
}

impl MeetingOwnerState {
    pub fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "active" => Self::Active,
            "paused" => Self::Paused,
            "stopping" => Self::Stopping,
            "completed" => Self::Completed,
            "saved" => Self::Saved,
            "failed" => Self::Failed,
            "interrupted" => Self::Interrupted,
            _ => return None,
        })
    }

    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Completed | Self::Saved | Self::Failed | Self::Interrupted
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CodingOwnerState {
    Queued,
    Running,
    CancelRequested,
    Settled,
    Failed,
    Interrupted,
    OutcomeUnknown,
}

impl CodingOwnerState {
    pub fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "queued" => Self::Queued,
            "running" => Self::Running,
            "cancel_requested" => Self::CancelRequested,
            "settled" => Self::Settled,
            "failed" => Self::Failed,
            "interrupted" => Self::Interrupted,
            "outcome_unknown" => Self::OutcomeUnknown,
            _ => return None,
        })
    }
}

/// Explicit `kind`/`state` wire shape: `{"kind":"meeting_session","state":"paused"}`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "state", rename_all = "snake_case")]
pub enum RuntimeOwnerState {
    MeetingSession(MeetingOwnerState),
    CodingJob(CodingOwnerState),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeStateView {
    pub reference: RuntimeRef,
    pub scope_key: String,
    pub owner_state: RuntimeOwnerState,
    pub phase: RuntimePhase,
    pub job_revision: Option<u64>,
    pub current_run_id: Option<String>,
    pub reported_complete: Option<bool>,
    pub owner_digest: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeFocusReason {
    ActiveProject,
    CurrentWork,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeFocus {
    pub reference: RuntimeRef,
    pub project_scope: String,
    pub reason: RuntimeFocusReason,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FrameNoticeCode {
    RuntimeUnavailable,
    RuntimeUnstable,
    RuntimeUnsupportedState,
    RuntimeCapacityOmitted,
    RuntimeBudgetOmitted,
    WorldProjectionStale,
    WorldPending,
    WorldCapacityOmitted,
    GraphNotRequested,
    NoticesTruncated,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FrameNotice {
    pub code: FrameNoticeCode,
    #[serde(default)]
    pub reference: Option<RuntimeRef>,
}

impl FrameNotice {
    pub fn global(code: FrameNoticeCode) -> Self {
        Self {
            code,
            reference: None,
        }
    }

    pub fn for_ref(code: FrameNoticeCode, reference: RuntimeRef) -> Self {
        Self {
            code,
            reference: Some(reference),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorldFrame {
    pub schema_version: i64,
    pub run_id: String,
    pub project_scope: String,
    pub captured_at_ms: i64,
    pub expires_at_ms: i64,
    pub graph: Option<WorldSliceV2>,
    pub runtime: Vec<RuntimeStateView>,
    pub runtime_focus: Vec<RuntimeFocus>,
    pub notices: Vec<FrameNotice>,
    pub truncated: bool,
}

impl WorldFrame {
    pub fn empty(
        run_id: &str,
        project_scope: &str,
        captured_at_ms: i64,
        expires_at_ms: i64,
    ) -> Self {
        Self {
            schema_version: WORLD_FRAME_SCHEMA_VERSION,
            run_id: run_id.to_string(),
            project_scope: project_scope.to_string(),
            captured_at_ms,
            expires_at_ms,
            graph: None,
            runtime: Vec::new(),
            runtime_focus: Vec::new(),
            notices: Vec::new(),
            truncated: false,
        }
    }

    pub fn encoded_len(&self) -> Result<usize, FrameError> {
        serde_json::to_vec(self)
            .map(|bytes| bytes.len())
            .map_err(|_| FrameError::InvalidInput)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FrameValidity {
    Current,
    Changed,
    Expired,
    ScopeDenied,
    Unavailable,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FrameStamp {
    pub ledger_revision: u64,
    pub input_epoch: u64,
    pub policy_revision: u64,
    pub scope_digest: String,
    pub owner_digests: Vec<(RuntimeRef, String)>,
    pub content_digest: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FrameError {
    InvalidInput,
    Limit,
    ScopeDenied,
    Changed,
    Expired,
    OwnerCorrupt,
    BudgetTooSmall,
    UnsupportedEvidenceContract,
    Unavailable,
    UnsupportedState,
    Other(String),
}

impl FrameError {
    pub fn code(&self) -> &str {
        match self {
            Self::InvalidInput => "frame-invalid-input",
            Self::Limit => "frame-limit",
            Self::ScopeDenied => "frame-scope-denied",
            Self::Changed => "frame-changed",
            Self::Expired => "frame-expired",
            Self::OwnerCorrupt => "frame-owner-corrupt",
            Self::BudgetTooSmall => "frame-budget-too-small",
            Self::UnsupportedEvidenceContract => "frame-unsupported-evidence-contract",
            Self::Unavailable => "runtime_unavailable",
            Self::UnsupportedState => "runtime_unsupported_state",
            Self::Other(code) => code.as_str(),
        }
    }
}

impl std::fmt::Display for FrameError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.code())
    }
}
impl std::error::Error for FrameError {}

impl From<String> for FrameError {
    fn from(value: String) -> Self {
        Self::Other(value)
    }
}

pub fn validate_frame_identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 160
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'))
}

pub fn runtime_scope_key(reference: &RuntimeRef) -> String {
    match reference.kind {
        RuntimeKind::MeetingSession => format!("resource:{}", reference.id),
        RuntimeKind::CodingJob => format!("task:{}", reference.id),
    }
}

/// Validate, canonicalize (kind/id order) and deduplicate runtime references.
/// The raw input is bounded before deduplication so a crafted request cannot
/// force unbounded work.
pub fn normalize_runtime_refs(refs: &[RuntimeRef]) -> Result<Vec<RuntimeRef>, FrameError> {
    if refs.len() > MAX_INPUT_RUNTIME_REFS {
        return Err(FrameError::Limit);
    }
    for reference in refs {
        if !validate_frame_identifier(&reference.id) {
            return Err(FrameError::InvalidInput);
        }
    }
    let mut canonical: Vec<RuntimeRef> = refs.to_vec();
    canonical.sort();
    canonical.dedup();
    if canonical.len() > MAX_RUNTIME_REFS {
        return Err(FrameError::Limit);
    }
    Ok(canonical)
}

/// TTL is 1..=1,000 ms. Zero is invalid; an over-limit request is capped, never
/// silently raised.
pub fn normalize_ttl(ttl_ms: u64) -> Result<u64, FrameError> {
    if ttl_ms == 0 {
        return Err(FrameError::InvalidInput);
    }
    Ok(ttl_ms.min(MAX_TTL_MS))
}

/// The lower bound is never raised; the upper bound is capped at 8,192.
pub fn effective_max_bytes(max_bytes: usize) -> usize {
    max_bytes.min(MAX_FRAME_BYTES)
}

/// Digest of everything the frame means except observation time and the
/// graph's own `as_of_ms`. Field order is fixed by the struct definitions.
pub fn content_digest(frame: &WorldFrame) -> Result<String, FrameError> {
    let mut canonical = frame.clone();
    canonical.captured_at_ms = 0;
    canonical.expires_at_ms = 0;
    if let Some(graph) = canonical.graph.as_mut() {
        graph.as_of_ms = 0;
    }
    let bytes = serde_json::to_vec(&canonical).map_err(|_| FrameError::InvalidInput)?;
    Ok(hex_sha256(&bytes))
}

pub fn hex_sha256(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn canonical_array(values: &[serde_json::Value]) -> String {
    serde_json::to_string(&values.to_vec()).unwrap_or_default()
}

/// Classify a stored frame against freshly re-read header + owner digests.
pub fn compare_stamp(old: &FrameStamp, new: &FrameStamp) -> FrameValidity {
    if old.scope_digest != new.scope_digest {
        return FrameValidity::Changed;
    }
    if old.ledger_revision != new.ledger_revision
        || old.input_epoch != new.input_epoch
        || old.policy_revision != new.policy_revision
        || old.owner_digests != new.owner_digests
        || old.content_digest != new.content_digest
    {
        return FrameValidity::Changed;
    }
    FrameValidity::Current
}

/// Strictly `captured_at <= now < expires_at`.
pub fn is_within_validity(captured_at_ms: i64, expires_at_ms: i64, now_ms: i64) -> bool {
    now_ms >= captured_at_ms && now_ms < expires_at_ms
}

// ---------------------------------------------------------------------------
// Meeting owner mapping (R3)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default)]
pub struct MeetingSnapshotInput<'a> {
    /// `None` when the session row is absent.
    pub db_status: Option<&'a str>,
    pub started_at: Option<&'a str>,
    pub ended_at: Option<&'a str>,
    pub saved_at: Option<&'a str>,
    /// The live runtime's current session id and phase, if any.
    pub live_session_id: Option<&'a str>,
    pub live_state: Option<MeetingLivePhase>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MeetingMapping {
    Unavailable,
    Unstable,
    Present {
        owner_state: MeetingOwnerState,
        phase: RuntimePhase,
        digest: String,
    },
}

fn live_state_str(state: MeetingLivePhase) -> &'static str {
    match state {
        MeetingLivePhase::Idle => "idle",
        MeetingLivePhase::Preflight => "preflight",
        MeetingLivePhase::Ready => "ready",
        MeetingLivePhase::Active => "active",
        MeetingLivePhase::Paused => "paused",
        MeetingLivePhase::Stopping => "stopping",
        MeetingLivePhase::Completed => "completed",
        MeetingLivePhase::Failed => "failed",
    }
}

fn meeting_digest(
    id: &str,
    status: &str,
    started_at: Option<&str>,
    ended_at: Option<&str>,
    saved_at: Option<&str>,
    live_session_id: Option<&str>,
    live_state: Option<MeetingLivePhase>,
) -> String {
    let live_id = live_session_id
        .map(serde_json::Value::from)
        .unwrap_or(serde_json::Value::Null);
    let live = live_state
        .map(|s| serde_json::Value::from(live_state_str(s)))
        .unwrap_or(serde_json::Value::Null);
    let values = [
        serde_json::Value::from("meeting_session"),
        serde_json::Value::from(id),
        serde_json::Value::from(status),
        started_at
            .map(serde_json::Value::from)
            .unwrap_or(serde_json::Value::Null),
        ended_at
            .map(serde_json::Value::from)
            .unwrap_or(serde_json::Value::Null),
        saved_at
            .map(serde_json::Value::from)
            .unwrap_or(serde_json::Value::Null),
        live_id,
        live,
    ];
    hex_sha256(canonical_array(&values).as_bytes())
}

/// Map the meeting owner's DB row and live snapshot to a runtime view. The
/// function is pure so the same rule can feed both the "before" and "after"
/// live snapshot checks.
pub fn map_meeting(id: &str, input: &MeetingSnapshotInput<'_>) -> MeetingMapping {
    let Some(status) = input.db_status else {
        return MeetingMapping::Unavailable;
    };
    if status == "discarded" {
        return MeetingMapping::Unavailable;
    }
    let Some(owner_state) = MeetingOwnerState::parse(status) else {
        return MeetingMapping::Unstable;
    };
    let same_live = input.live_session_id == Some(id);
    let live = if same_live { input.live_state } else { None };

    if owner_state.is_terminal() {
        // Terminal rows ignore a different/absent live session. A live session
        // that claims the same id is still progressing is an inconsistency.
        if same_live
            && matches!(
                input.live_state,
                Some(
                    MeetingLivePhase::Active
                        | MeetingLivePhase::Paused
                        | MeetingLivePhase::Stopping
                )
            )
        {
            return MeetingMapping::Unstable;
        }
        // Normalize unrelated live information to null/null. A same-id live
        // session (in a non-progressing state) keeps its id and state.
        let live_session_id = if same_live { Some(id) } else { None };
        let digest = meeting_digest(
            id,
            status,
            input.started_at,
            input.ended_at,
            input.saved_at,
            live_session_id,
            live,
        );
        return MeetingMapping::Present {
            owner_state,
            phase: RuntimePhase::Terminal,
            digest,
        };
    }

    match (owner_state, live) {
        (MeetingOwnerState::Active, Some(MeetingLivePhase::Active)) => MeetingMapping::Present {
            owner_state,
            phase: RuntimePhase::Running,
            digest: meeting_digest(
                id,
                status,
                input.started_at,
                input.ended_at,
                input.saved_at,
                Some(id),
                live,
            ),
        },
        (MeetingOwnerState::Paused, Some(MeetingLivePhase::Paused)) => MeetingMapping::Present {
            owner_state,
            phase: RuntimePhase::Paused,
            digest: meeting_digest(
                id,
                status,
                input.started_at,
                input.ended_at,
                input.saved_at,
                Some(id),
                live,
            ),
        },
        (MeetingOwnerState::Active, Some(MeetingLivePhase::Stopping))
        | (MeetingOwnerState::Paused, Some(MeetingLivePhase::Stopping)) => {
            MeetingMapping::Present {
                owner_state: MeetingOwnerState::Stopping,
                phase: RuntimePhase::Stopping,
                digest: meeting_digest(
                    id,
                    status,
                    input.started_at,
                    input.ended_at,
                    input.saved_at,
                    Some(id),
                    live,
                ),
            }
        }
        _ => MeetingMapping::Unstable,
    }
}

// ---------------------------------------------------------------------------
// Coding owner mapping (R4)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default)]
pub struct CodingSnapshotInput<'a> {
    pub job_id: &'a str,
    pub conversation_id: &'a str,
    pub revision: u64,
    pub job_state: &'a str,
    pub current_run_id: Option<&'a str>,
    pub run_state: Option<&'a str>,
    pub delivery: Option<&'a str>,
    pub ended_at: Option<&'a str>,
    pub reported_complete: Option<bool>,
    /// `(source_id, version, project_scope)` sorted by id.
    pub source_versions: Vec<(String, u64, String)>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CodingMapping {
    UnsupportedState,
    Present {
        owner_state: CodingOwnerState,
        phase: RuntimePhase,
        digest: String,
    },
}

fn coding_digest(input: &CodingSnapshotInput<'_>) -> String {
    let sources: Vec<serde_json::Value> = input
        .source_versions
        .iter()
        .map(|(id, version, project)| {
            serde_json::Value::Array(vec![
                serde_json::Value::from(id.as_str()),
                serde_json::Value::from(*version),
                serde_json::Value::from(project.as_str()),
            ])
        })
        .collect();
    let values = [
        serde_json::Value::from("coding_job"),
        serde_json::Value::from(input.job_id),
        serde_json::Value::from(input.conversation_id),
        serde_json::Value::from(input.revision),
        serde_json::Value::from(input.job_state),
        input
            .current_run_id
            .map(serde_json::Value::from)
            .unwrap_or(serde_json::Value::Null),
        input
            .run_state
            .map(serde_json::Value::from)
            .unwrap_or(serde_json::Value::Null),
        input
            .delivery
            .map(serde_json::Value::from)
            .unwrap_or(serde_json::Value::Null),
        input
            .ended_at
            .map(serde_json::Value::from)
            .unwrap_or(serde_json::Value::Null),
        input
            .reported_complete
            .map(serde_json::Value::from)
            .unwrap_or(serde_json::Value::Null),
        serde_json::Value::Array(sources),
    ];
    hex_sha256(canonical_array(&values).as_bytes())
}

pub fn map_coding(input: &CodingSnapshotInput<'_>) -> CodingMapping {
    let Some(owner_state) = CodingOwnerState::parse(input.job_state) else {
        return CodingMapping::UnsupportedState;
    };
    let phase = match owner_state {
        CodingOwnerState::Queued => RuntimePhase::Starting,
        CodingOwnerState::Running => {
            if input.run_state == Some("running") && input.delivery == Some("accepted") {
                RuntimePhase::Running
            } else {
                RuntimePhase::Unknown
            }
        }
        CodingOwnerState::CancelRequested => RuntimePhase::Stopping,
        CodingOwnerState::Settled | CodingOwnerState::Failed | CodingOwnerState::Interrupted => {
            RuntimePhase::Terminal
        }
        CodingOwnerState::OutcomeUnknown => RuntimePhase::Unknown,
    };
    let digest = coding_digest(input);
    CodingMapping::Present {
        owner_state,
        phase,
        digest,
    }
}

/// Focus is only attached to a live, running unit. Terminal states never
/// synthesise a new `active_project` / `current_work`.
pub fn focus_for(view: &RuntimeStateView, project_scope: &str) -> Option<RuntimeFocus> {
    if view.phase != RuntimePhase::Running {
        return None;
    }
    let reason = match view.reference.kind {
        RuntimeKind::MeetingSession => RuntimeFocusReason::ActiveProject,
        RuntimeKind::CodingJob => RuntimeFocusReason::CurrentWork,
    };
    Some(RuntimeFocus {
        reference: view.reference.clone(),
        project_scope: project_scope.to_string(),
        reason,
    })
}

// ---------------------------------------------------------------------------
// Bounded assembly (R7)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct RuntimeUnit {
    pub view: RuntimeStateView,
    pub focus: Option<RuntimeFocus>,
}

pub struct FrameAssembly<'a> {
    pub run_id: &'a str,
    pub project_scope: &'a str,
    pub captured_at_ms: i64,
    pub expires_at_ms: i64,
    pub max_bytes: usize,
    pub graph: Option<WorldSliceV2>,
    pub runtime: Vec<RuntimeUnit>,
    pub notices: Vec<FrameNotice>,
}

fn sort_notices(notices: &mut Vec<FrameNotice>) {
    notices.sort();
    notices.dedup();
}

fn normalize_notices(notices: Vec<FrameNotice>, limit: usize) -> Vec<FrameNotice> {
    let mut notices = notices;
    sort_notices(&mut notices);
    if notices.len() > limit {
        notices.truncate(limit.saturating_sub(1));
        notices.push(FrameNotice::global(FrameNoticeCode::NoticesTruncated));
    }
    notices
}

fn push_notice(frame: &mut WorldFrame, notice: FrameNotice) {
    if !frame.notices.contains(&notice) {
        frame.notices.push(notice);
    }
}

/// Assemble a bounded WorldFrame. Whole runtime units and the whole graph are
/// adopted or omitted; conditions, evidence and notices are never partially
/// stripped.
pub fn assemble_frame(input: FrameAssembly<'_>) -> Result<WorldFrame, FrameError> {
    let final_cap = effective_max_bytes(input.max_bytes);
    let mut frame = WorldFrame::empty(
        input.run_id,
        input.project_scope,
        input.captured_at_ms,
        input.expires_at_ms,
    );
    // Reserve one notice slot for a graph/runtime omission added below.
    frame.notices = normalize_notices(input.notices, MAX_NOTICES.saturating_sub(1));
    frame.truncated = false;
    if frame.encoded_len()? > final_cap {
        return Err(FrameError::BudgetTooSmall);
    }

    let mut units = input.runtime;
    units.sort_by(|a, b| a.view.reference.cmp(&b.view.reference));
    let mut runtime_bytes = 0usize;
    let mut omitted_runtime = false;
    for unit in units {
        let mut probe = frame.clone();
        probe.runtime.push(unit.view.clone());
        if let Some(focus) = &unit.focus {
            probe.runtime_focus.push(focus.clone());
        }
        let probe_len = probe.encoded_len()?;
        let unit_len = probe_len.saturating_sub(frame.encoded_len()?);
        if probe.runtime.len() > MAX_RUNTIME_REFS
            || runtime_bytes + unit_len > MAX_RUNTIME_BYTES
            || probe_len > final_cap
        {
            omitted_runtime = true;
            continue;
        }
        runtime_bytes += unit_len;
        frame = probe;
    }
    if omitted_runtime {
        frame.truncated = true;
        push_notice(
            &mut frame,
            FrameNotice::global(FrameNoticeCode::RuntimeBudgetOmitted),
        );
    }

    if let Some(graph) = input.graph {
        let mut probe = frame.clone();
        probe.graph = Some(graph);
        let node_total =
            probe.graph.as_ref().map(|g| g.nodes.len()).unwrap_or(0) + probe.runtime.len();
        let probe_len = probe.encoded_len()?;
        let graph_len = probe_len.saturating_sub(frame.encoded_len()?);
        if node_total > MAX_TOTAL_NODES || graph_len > MAX_GRAPH_BYTES || probe_len > final_cap {
            frame.truncated = true;
            push_notice(
                &mut frame,
                FrameNotice::global(FrameNoticeCode::WorldCapacityOmitted),
            );
        } else {
            frame = probe;
        }
    }

    // Focus must reference an adopted runtime view.
    let adopted: Vec<RuntimeRef> = frame.runtime.iter().map(|v| v.reference.clone()).collect();
    frame
        .runtime_focus
        .retain(|focus| adopted.contains(&focus.reference));

    frame.notices = normalize_notices(std::mem::take(&mut frame.notices), MAX_NOTICES);

    // Notice handling order (R7): drop the whole graph first, then runtime units
    // in kind/id descending order. Conditions, evidence and focus are never
    // partially stripped from an adopted unit.
    while frame.encoded_len()? > final_cap {
        if frame.graph.is_some() {
            frame.graph = None;
            frame.truncated = true;
            push_notice(
                &mut frame,
                FrameNotice::global(FrameNoticeCode::WorldCapacityOmitted),
            );
            frame.notices = normalize_notices(std::mem::take(&mut frame.notices), MAX_NOTICES);
            continue;
        }
        let Some(dropped) = frame.runtime.pop() else {
            return Err(FrameError::BudgetTooSmall);
        };
        frame
            .runtime_focus
            .retain(|focus| focus.reference != dropped.reference);
        frame.truncated = true;
        push_notice(
            &mut frame,
            FrameNotice::global(FrameNoticeCode::RuntimeBudgetOmitted),
        );
        frame.notices = normalize_notices(std::mem::take(&mut frame.notices), MAX_NOTICES);
    }

    if frame.encoded_len()? > final_cap {
        return Err(FrameError::BudgetTooSmall);
    }
    Ok(frame)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn meeting_view(id: &str, phase: RuntimePhase) -> RuntimeUnit {
        let reference = RuntimeRef {
            kind: RuntimeKind::MeetingSession,
            id: id.into(),
        };
        let view = RuntimeStateView {
            scope_key: runtime_scope_key(&reference),
            reference: reference.clone(),
            owner_state: RuntimeOwnerState::MeetingSession(MeetingOwnerState::Active),
            phase,
            job_revision: None,
            current_run_id: None,
            reported_complete: None,
            owner_digest: "d".into(),
        };
        RuntimeUnit {
            focus: focus_for(&view, "project:p"),
            view,
        }
    }

    fn assembly(max_bytes: usize, runtime: Vec<RuntimeUnit>) -> FrameAssembly<'static> {
        FrameAssembly {
            run_id: "run1",
            project_scope: "project:p",
            captured_at_ms: 1_000,
            expires_at_ms: 2_000,
            max_bytes,
            graph: None,
            runtime,
            notices: Vec::new(),
        }
    }

    #[test]
    fn m2_02_frame_roundtrips_and_rejects_unknown_fields() {
        let frame = WorldFrame::empty("run1", "project:p", 1_000, 2_000);
        let bytes = serde_json::to_vec(&frame).unwrap();
        let decoded: WorldFrame = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(decoded, frame);
        let mut value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        value["unexpected"] = serde_json::json!(true);
        assert!(serde_json::from_value::<WorldFrame>(value).is_err());
    }

    #[test]
    fn m2_02_owner_state_wire_shape_is_explicit() {
        let owner = RuntimeOwnerState::MeetingSession(MeetingOwnerState::Paused);
        assert_eq!(
            serde_json::to_value(owner).unwrap(),
            serde_json::json!({"kind":"meeting_session","state":"paused"})
        );
        let unknown = serde_json::json!({"kind":"meeting_session","state":"bogus"});
        assert!(serde_json::from_value::<RuntimeOwnerState>(unknown).is_err());
    }

    #[test]
    fn m2_03_normalization_dedups_and_sorts() {
        let refs = vec![
            RuntimeRef {
                kind: RuntimeKind::CodingJob,
                id: "j1".into(),
            },
            RuntimeRef {
                kind: RuntimeKind::MeetingSession,
                id: "m1".into(),
            },
            RuntimeRef {
                kind: RuntimeKind::CodingJob,
                id: "j1".into(),
            },
        ];
        let normalized = normalize_runtime_refs(&refs).unwrap();
        assert_eq!(normalized.len(), 2);
        assert_eq!(normalized[0].kind, RuntimeKind::MeetingSession);
        assert_eq!(normalized[1].kind, RuntimeKind::CodingJob);
    }

    #[test]
    fn m2_03_nine_distinct_refs_are_a_limit() {
        let refs: Vec<_> = (0..9)
            .map(|i| RuntimeRef {
                kind: RuntimeKind::CodingJob,
                id: format!("j{i}"),
            })
            .collect();
        assert_eq!(normalize_runtime_refs(&refs), Err(FrameError::Limit));
    }

    #[test]
    fn m2_03_ttl_zero_is_invalid_and_over_limit_is_capped() {
        assert_eq!(normalize_ttl(0), Err(FrameError::InvalidInput));
        assert_eq!(normalize_ttl(1_001).unwrap(), 1_000);
        assert_eq!(normalize_ttl(1_000).unwrap(), 1_000);
        assert_eq!(normalize_ttl(1).unwrap(), 1);
    }

    #[test]
    fn m2_03_max_bytes_lower_bound_is_not_raised() {
        assert_eq!(effective_max_bytes(1), 1);
        assert_eq!(effective_max_bytes(100_000), MAX_FRAME_BYTES);
    }

    #[test]
    fn m2_04_content_digest_ignores_observation_time_only() {
        let mut frame = WorldFrame::empty("run1", "project:p", 1_000, 2_000);
        let first = content_digest(&frame).unwrap();
        frame.captured_at_ms = 500;
        frame.expires_at_ms = 9_999;
        assert_eq!(content_digest(&frame).unwrap(), first, "time is excluded");
        frame.truncated = true;
        assert_ne!(
            content_digest(&frame).unwrap(),
            first,
            "state changes digest"
        );
    }

    #[test]
    fn m2_04_validity_boundaries() {
        assert!(!is_within_validity(1_000, 2_000, 999));
        assert!(is_within_validity(1_000, 2_000, 1_999));
        assert!(!is_within_validity(1_000, 2_000, 2_000));
    }

    #[test]
    fn m2_09_meeting_mapping_covers_r3_table() {
        let base = MeetingSnapshotInput {
            db_status: Some("active"),
            started_at: Some("100"),
            ended_at: None,
            saved_at: None,
            live_session_id: Some("m1"),
            live_state: Some(MeetingLivePhase::Active),
        };
        assert_eq!(
            map_meeting("m1", &base),
            MeetingMapping::Present {
                owner_state: MeetingOwnerState::Active,
                phase: RuntimePhase::Running,
                digest: match map_meeting("m1", &base) {
                    MeetingMapping::Present { digest, .. } => digest,
                    _ => unreachable!(),
                },
            }
        );
        assert!(matches!(
            map_meeting(
                "m1",
                &MeetingSnapshotInput {
                    live_state: Some(MeetingLivePhase::Paused),
                    ..base.clone()
                }
            ),
            MeetingMapping::Unstable
        ));
        let paused = MeetingSnapshotInput {
            db_status: Some("paused"),
            live_state: Some(MeetingLivePhase::Paused),
            ..base.clone()
        };
        assert!(matches!(
            map_meeting("m1", &paused),
            MeetingMapping::Present {
                phase: RuntimePhase::Paused,
                ..
            }
        ));
        let stopping = MeetingSnapshotInput {
            db_status: Some("paused"),
            live_state: Some(MeetingLivePhase::Stopping),
            ..base.clone()
        };
        assert!(matches!(
            map_meeting("m1", &stopping),
            MeetingMapping::Present {
                phase: RuntimePhase::Stopping,
                ..
            }
        ));
        let terminal = MeetingSnapshotInput {
            db_status: Some("saved"),
            ended_at: Some("900"),
            saved_at: Some("950"),
            live_session_id: None,
            live_state: None,
            ..base.clone()
        };
        assert!(matches!(
            map_meeting("m1", &terminal),
            MeetingMapping::Present {
                phase: RuntimePhase::Terminal,
                owner_state: MeetingOwnerState::Saved,
                ..
            }
        ));
        // A different live session must not change the terminal digest.
        let other_live = MeetingSnapshotInput {
            live_session_id: Some("m2"),
            live_state: Some(MeetingLivePhase::Active),
            ..terminal.clone()
        };
        assert_eq!(map_meeting("m1", &terminal), map_meeting("m1", &other_live));
        assert_eq!(
            map_meeting(
                "m1",
                &MeetingSnapshotInput {
                    db_status: Some("discarded"),
                    ..base.clone()
                }
            ),
            MeetingMapping::Unavailable
        );
        assert_eq!(
            map_meeting("m1", &MeetingSnapshotInput::default()),
            MeetingMapping::Unavailable
        );
        assert!(matches!(
            map_meeting(
                "m1",
                &MeetingSnapshotInput {
                    db_status: Some("active"),
                    live_session_id: None,
                    live_state: None,
                    ..base
                }
            ),
            MeetingMapping::Unstable
        ));
    }

    #[test]
    fn m2_12_coding_mapping_and_focus() {
        let input = CodingSnapshotInput {
            job_id: "j1",
            conversation_id: "c1",
            revision: 7,
            job_state: "running",
            current_run_id: Some("r1"),
            run_state: Some("running"),
            delivery: Some("accepted"),
            ended_at: None,
            reported_complete: None,
            source_versions: vec![("s1".into(), 1, "project:p".into())],
        };
        let CodingMapping::Present { phase, digest, .. } = map_coding(&input) else {
            panic!("running job maps");
        };
        assert_eq!(phase, RuntimePhase::Running);
        let reference = RuntimeRef {
            kind: RuntimeKind::CodingJob,
            id: "j1".into(),
        };
        let view = RuntimeStateView {
            scope_key: runtime_scope_key(&reference),
            reference: reference.clone(),
            owner_state: RuntimeOwnerState::CodingJob(CodingOwnerState::Running),
            phase,
            job_revision: Some(7),
            current_run_id: Some("r1".into()),
            reported_complete: None,
            owner_digest: digest.clone(),
        };
        let focus = focus_for(&view, "project:p").expect("running coding job gets focus");
        assert_eq!(focus.reason, RuntimeFocusReason::CurrentWork);
        assert_eq!(focus.reference, reference);

        // revision unchanged, state changed -> digest changes.
        let canceled = CodingSnapshotInput {
            job_state: "cancel_requested",
            ..input.clone()
        };
        assert_ne!(coding_digest(&input), coding_digest(&canceled));
        let CodingMapping::Present { phase, .. } = map_coding(&canceled) else {
            panic!("cancel maps");
        };
        assert_eq!(phase, RuntimePhase::Stopping);
        assert!(focus_for(
            &RuntimeStateView {
                owner_state: RuntimeOwnerState::CodingJob(CodingOwnerState::CancelRequested),
                phase,
                ..view.clone()
            },
            "project:p"
        )
        .is_none());

        let settled = CodingSnapshotInput {
            job_state: "settled",
            ..input.clone()
        };
        let CodingMapping::Present { phase, .. } = map_coding(&settled) else {
            panic!("settled maps");
        };
        assert_eq!(phase, RuntimePhase::Terminal);

        let unknown = CodingSnapshotInput {
            job_state: "bogus",
            ..input.clone()
        };
        assert_eq!(map_coding(&unknown), CodingMapping::UnsupportedState);
    }

    #[test]
    fn m2_14_runtime_units_are_bounded_by_bytes_and_count() {
        let units: Vec<_> = (0..8)
            .map(|i| meeting_view(&format!("m{i}"), RuntimePhase::Running))
            .collect();
        let frame = assemble_frame(assembly(MAX_FRAME_BYTES, units)).unwrap();
        assert!(frame.runtime.len() <= MAX_RUNTIME_REFS);
        assert_eq!(frame.runtime.len(), frame.runtime_focus.len());
        assert!(frame.encoded_len().unwrap() <= MAX_FRAME_BYTES);
        if frame.runtime.len() < 8 {
            assert!(frame.truncated);
            assert!(frame
                .notices
                .iter()
                .any(|n| n.code == FrameNoticeCode::RuntimeBudgetOmitted));
        }
        // Every adopted unit keeps a whole focus; nothing is half-included.
        for focus in &frame.runtime_focus {
            assert!(frame.runtime.iter().any(|v| v.reference == focus.reference));
        }
    }

    #[test]
    fn m2_14_one_byte_budget_fails() {
        assert_eq!(
            assemble_frame(assembly(1, Vec::new())).unwrap_err(),
            FrameError::BudgetTooSmall
        );
    }

    #[test]
    fn m2_14_focus_never_dangles() {
        let mut unit = meeting_view("m1", RuntimePhase::Terminal);
        unit.focus = Some(RuntimeFocus {
            reference: RuntimeRef {
                kind: RuntimeKind::MeetingSession,
                id: "ghost".into(),
            },
            project_scope: "project:p".into(),
            reason: RuntimeFocusReason::ActiveProject,
        });
        let frame = assemble_frame(assembly(MAX_FRAME_BYTES, vec![unit])).unwrap();
        assert!(frame.runtime_focus.is_empty());
    }
}
