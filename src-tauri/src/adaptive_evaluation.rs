//! Host-owned evaluation records. UI scores cannot promote an artifact.
use crate::adaptive_improvement::{
    apply_evaluation_gate, evaluate_paired, EvaluationGate, PairedEvaluationSample,
};
use crate::{database_error, new_id};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use ts_rs::TS;
include!("adaptive_evaluation.d/01.rs");
include!("adaptive_evaluation.d/02.rs");
