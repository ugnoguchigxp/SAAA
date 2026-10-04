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
#[cfg(any(test, feature = "offline-contracts"))]
pub use history_limit::{apply_retention, persist_entry};
pub use history_limit::{
    clear_history, evaluation_summary, feedback_queue, latest_entry, list_history, load_settings,
    persist_entry_with_retention, persist_quality_window, quality_metrics, save_enabled,
    submit_feedback,
};
use validate_ledger_entry::{validate_ledger_entry, validate_quality_counters};
