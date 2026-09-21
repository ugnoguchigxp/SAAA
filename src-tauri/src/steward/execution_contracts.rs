//! Typed execution outcomes. Status strings are never interpreted ad hoc in UI code.
use serde::{Deserialize, Serialize};
use ts_rs::TS;

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum TaskState {
    Queued,
    Dispatching,
    Running,
    AwaitingDependency,
    AwaitingUser,
    Verifying,
    Done,
    Failed,
    Cancelled,
    OutcomeUnknown,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum GoalProgress {
    Queued,
    Running,
    AwaitingUser,
    Done,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ProposeDecision {
    Accepted,
    RequiresConfirmation,
    Clarify,
    Rejected,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum DispatchOutcome {
    LocalAccepted,
    Busy,
    SettingsOff,
    ForegroundBusy,
    ScopeRevoked,
    Cancelled,
    Failed,
    OutcomeUnknown,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum VerifierOutcome {
    Pass,
    Fail,
    Missing,
    AwaitingUser,
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WorkProposeResult {
    pub decision: ProposeDecision,
    pub proposal_id: Option<String>,
    pub goal_id: Option<String>,
    pub task_id: Option<String>,
    pub reason: String,
    pub duplicate: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub(crate) struct StewardGoalView {
    pub goal_id: String,
    pub summary: String,
    pub authority_status: String,
    pub progress: GoalProgress,
    pub workspace_id: String,
    pub operations: String,
    pub verifier: String,
    pub budget_runs: i64,
    pub budget_ms: i64,
    pub notify: String,
    pub awaiting_reason: Option<String>,
    pub report_revision: i64,
    pub unsupported_profile: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DelegatedReportCommitted {
    pub conversation_id: String,
    pub message_id: String,
    pub report_revision: i64,
    pub cursor: i64,
}

pub(crate) fn typescript_bindings() -> String {
    use ts_rs::{Config, TS};
    let types = [
        TaskState::decl(&Config::default()),
        GoalProgress::decl(&Config::default()),
        ProposeDecision::decl(&Config::default()),
        DispatchOutcome::decl(&Config::default()),
        VerifierOutcome::decl(&Config::default()),
        WorkProposeResult::decl(&Config::default()),
        StewardGoalView::decl(&Config::default()),
        DelegatedReportCommitted::decl(&Config::default()),
    ]
    .join("\n\n");
    format!("// Generated from steward/execution_contracts.rs. Do not edit.\n{types}\n")
}
