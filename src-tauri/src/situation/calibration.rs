use super::contracts::{validate_calibration_parameters, CalibrationParameters, SituationScene};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::json;
include!("calibration.d/01.rs");
include!("calibration.d/02.rs");
