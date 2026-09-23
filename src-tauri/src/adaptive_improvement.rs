//! Conservative, local policy adaptation.
//!
//! This is deliberately a *re-ordering* layer: callers provide candidates that have already
//! passed their domain's permission and capability checks.  An adaptive policy can therefore
//! never introduce a tool, recipe, plan step, or notification channel.
use chrono::{Datelike, Timelike};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::sync::Arc;
#[path = "adaptive_improvement/domain.rs"]
mod domain;
#[path = "adaptive_improvement/materialize_dirty.rs"]
mod materialize_dirty;
use domain::now_ms;
pub(crate) use domain::{
    evaluate_paired, migrate, paired_bootstrap, record_decision, record_outcome,
    record_outcome_in_transaction, start_worker, train_candidate_artifacts, DecisionObservation,
    Domain, EvaluationGate, EvaluationSummary, PairedEvaluationSample, PairedInterval,
};
pub(crate) use materialize_dirty::{
    activate, apply_evaluation_gate, approve_shadow, choose, create_artifact, digest,
    fingerprint_for, invalidate_source, materialize_dirty, revoke_override,
    rollback_active_to_rules, set_override,
};
#[cfg(test)]
#[path = "adaptive_improvement/tests.rs"]
mod tests;
