use super::contracts::{validate_calibration_parameters, CalibrationParameters, SituationScene};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::json;
#[path = "calibration/replay_sample_v1.rs"]
mod replay_sample_v1;
pub use replay_sample_v1::{
    active_profile, candidates, create_candidate, decide, latest_run, profile_by_id,
    replay_metrics, save_run, CalibrationProfile, CalibrationRun,
};
#[cfg(any(test, feature = "offline-contracts"))]
pub use replay_sample_v1::{profile_by_status, FIXTURE_SET_VERSION};
#[cfg(any(test, feature = "offline-contracts"))]
use replay_sample_v1::{replay_scenario, ReplaySampleV1, ReplaySummary};
#[cfg(test)]
#[path = "calibration/tests.rs"]
mod tests;
