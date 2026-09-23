//! SQL access for the tool-selection ledger. Every function takes a `&Connection` so it can run
//! inside the existing `SqliteWriter` transaction; there is no second connection pool.

use super::contracts::*;
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::Value;
#[path = "repository/epochs.rs"]
mod epochs;
#[path = "repository/insert_decision.rs"]
mod insert_decision;
pub(crate) use epochs::effect_for_backend_key;
pub use epochs::{
    bump_epochs, delete_fts, epochs, insert_revision, set_current_revision, upsert_embedding,
    upsert_fts, upsert_grant, upsert_source, upsert_tool, upsert_usage_page,
    upsert_usage_page_bounded, EligibleRevision, Epochs, NewFeedback, NewRevision, NewRule,
    NewTool, RevisionRow, ToolRow,
};
pub use epochs::{
    eligible_revisions, lexical_candidates, load_embeddings, revision_by_id, set_source_enabled,
    tool_by_id, tool_id_by_name,
};
pub use insert_decision::{
    active_rules, decision_by_id, finish_invocation, grant_exists, insert_decision,
    insert_feedback_if_absent, insert_invocation, insert_rule, insert_rule_source_binding,
    invalidate_source_rule_bindings, recent_decisions, revoke_matching_soft_rules,
    set_satisfaction_for_decision, source_binding_hash, supersede_soft_rules, truncate_utf8,
    update_feedback_status, usage_page,
};
