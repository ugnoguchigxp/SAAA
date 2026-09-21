// Generated from steward/execution_contracts.rs. Do not edit.
type TaskState = "queued" | "dispatching" | "running" | "awaiting_dependency" | "awaiting_user" | "verifying" | "done" | "failed" | "cancelled" | "outcome_unknown";

type GoalProgress = "queued" | "running" | "awaiting_user" | "done" | "failed" | "cancelled";

type ProposeDecision = "accepted" | "requires_confirmation" | "clarify" | "rejected";

type DispatchOutcome = "local_accepted" | "busy" | "settings_off" | "foreground_busy" | "scope_revoked" | "cancelled" | "failed" | "outcome_unknown";

type VerifierOutcome = "pass" | "fail" | "missing" | "awaiting_user" | "unknown";

type WorkProposeResult = { decision: ProposeDecision, proposalId: string | null, goalId: string | null, taskId: string | null, reason: string, duplicate: boolean, };

type StewardGoalView = { goalId: string, summary: string, authorityStatus: string, progress: GoalProgress, workspaceId: string, operations: string, verifier: string, budgetRuns: bigint, budgetMs: bigint, notify: string, awaitingReason: string | null, reportRevision: bigint, unsupportedProfile: boolean, };

type DelegatedReportCommitted = { conversationId: string, messageId: string, reportRevision: bigint, cursor: bigint, };
