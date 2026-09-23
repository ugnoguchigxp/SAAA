//! Host-owned evaluation records. UI scores cannot promote an artifact.
use crate::adaptive_improvement::{
    apply_evaluation_gate, evaluate_paired, EvaluationGate, PairedEvaluationSample,
};
use crate::{database_error, new_id};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use ts_rs::TS;
#[path = "adaptive_evaluation/evaluation_pair.rs"]
pub(crate) mod evaluation_pair;
pub(crate) use evaluation_pair::{EVALUATOR_VERSION, EvaluationPair, EvaluationBundle, EvaluationView, resource_ratio, import_bundle, list_views, approve, activate, AdaptiveEvaluateInput, AdaptiveArtifactAction, list_adaptive_evaluations, import_adaptive_evaluation, approve_adaptive_artifact, activate_adaptive_artifact, typescript_bindings};
use evaluation_pair::{digest};
#[cfg(test)]
use crate::adaptive_improvement::{self, Domain};
#[cfg(test)]
use crate::initialize_database;
#[cfg(test)]
#[path = "adaptive_evaluation/tests.rs"]
mod tests;
