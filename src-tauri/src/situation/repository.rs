use super::contracts::{
    QualityWindowCounters, ShadowDecision, SituationEvaluationSummary, SituationFeedback,
    SituationFeedbackInput, SituationLedgerEntry, SituationQualityMetrics,
    SituationRuntimeSettings, SituationState,
};
use rusqlite::{params, Connection, OptionalExtension};
use serde::de::DeserializeOwned;
#[path = "repository/history_limit.rs"]
mod history_limit;
#[path = "repository/validate_ledger_entry.rs"]
mod validate_ledger_entry;
pub use history_limit::{load_settings, save_enabled, persist_entry, persist_entry_with_retention, persist_quality_window, apply_retention, quality_metrics, list_history, latest_entry, feedback_queue, evaluation_summary, submit_feedback, clear_history};
use validate_ledger_entry::{validate_ledger_entry, validate_quality_counters};
