#![allow(dead_code)]

use serde::Deserialize;
use serde_json::{Map, Value};
use std::collections::{HashMap, HashSet};
#[path = "contracts/operation.rs"]
mod operation;
#[path = "contracts/ensure_unique.rs"]
mod ensure_unique;
pub(crate) use operation::{HOST_PROTOCOL, MAX_REQUEST_BYTES, Operation, HostRequest, HostErrorCode, HostResponse, HostOutcome, OperationResult, InspectResult, CapabilityManifest, WasmContract, SourceRequirement, NotRun, CapabilityReport, ReportStatus, CaseOrigin, CaseObservation, CaseResult, ReportRequirement, parse_response};
pub(crate) use ensure_unique::{ensure_unique, safe_flat_path, is_hash};
