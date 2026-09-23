#![allow(dead_code)]

use serde::Deserialize;
use serde_json::{Map, Value};
use std::collections::{HashMap, HashSet};
#[path = "contracts/ensure_unique.rs"]
mod ensure_unique;
#[path = "contracts/operation.rs"]
mod operation;
pub(crate) use ensure_unique::{ensure_unique, is_hash, safe_flat_path};
pub(crate) use operation::{
    parse_response, CapabilityManifest, CapabilityReport, CaseObservation, CaseOrigin, CaseResult,
    HostErrorCode, HostOutcome, HostRequest, HostResponse, InspectResult, NotRun, Operation,
    OperationResult, ReportRequirement, ReportStatus, SourceRequirement, WasmContract,
    HOST_PROTOCOL, MAX_REQUEST_BYTES,
};
