//! SQL access for the tool-selection ledger. Every function takes a `&Connection` so it can run
//! inside the existing `SqliteWriter` transaction; there is no second connection pool.

use super::contracts::*;
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::Value;
#[path = "repository/epochs.rs"]
mod epochs;
#[path = "repository/insert_decision.rs"]
mod insert_decision;
pub use epochs::{Epochs, ToolRow, RevisionRow, EligibleRevision, NewTool, NewRevision, NewFeedback, NewRule, epochs, bump_epochs, upsert_source, upsert_tool, insert_revision, set_current_revision, upsert_usage_page, upsert_usage_page_bounded, upsert_grant, upsert_fts, delete_fts, upsert_embedding};
pub use epochs::{set_source_enabled, tool_by_id, tool_id_by_name, revision_by_id, eligible_revisions, lexical_candidates, load_embeddings};
pub(crate) use epochs::{effect_for_backend_key};
pub use insert_decision::{insert_decision, decision_by_id, insert_invocation, finish_invocation, set_satisfaction_for_decision, insert_feedback_if_absent, update_feedback_status, insert_rule, source_binding_hash, insert_rule_source_binding, invalidate_source_rule_bindings, supersede_soft_rules, active_rules, revoke_matching_soft_rules, grant_exists, usage_page, recent_decisions, truncate_utf8};
