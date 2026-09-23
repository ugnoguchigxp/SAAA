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
pub(crate) use domain::{Domain, EvaluationGate, PairedInterval, PairedEvaluationSample, EvaluationSummary, evaluate_paired, paired_bootstrap, DecisionObservation, migrate, start_worker, train_candidate_artifacts, record_decision, record_outcome, record_outcome_in_transaction};
use domain::{now_ms};
pub(crate) use materialize_dirty::{materialize_dirty, set_override, revoke_override, choose, fingerprint_for, digest, create_artifact, apply_evaluation_gate, approve_shadow, activate, rollback_active_to_rules, invalidate_source};
#[cfg(test)]
#[path = "adaptive_improvement/tests.rs"]
mod tests;
