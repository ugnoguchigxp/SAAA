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
#[cfg(test)]
use crate::adaptive_improvement::{self, Domain};
#[cfg(test)]
use crate::initialize_database;
use evaluation_pair::digest;
pub(crate) use evaluation_pair::{
    activate, activate_adaptive_artifact, approve, approve_adaptive_artifact,
    import_adaptive_evaluation, import_bundle, list_adaptive_evaluations, list_views,
    resource_ratio, typescript_bindings, AdaptiveArtifactAction, AdaptiveEvaluateInput,
    EvaluationBundle, EvaluationPair, EvaluationView, EVALUATOR_VERSION,
};
#[cfg(test)]
#[path = "adaptive_evaluation/tests.rs"]
mod tests;
