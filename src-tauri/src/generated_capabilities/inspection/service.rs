#![allow(private_interfaces)]

//! Execution-time inspection service (plan 3, C05/C06).
//!
//! The service never trusts a client-supplied path or the currently active revision. It resolves
//! the call owner from the persisted table, fixes the revision recorded on the call, re-checks the
//! managed package, runs the fixed inspector, compares the trusted projection with Wasm over the
//! full boolean domain, and only then publishes the TypeScript and report under
//! `<data>/generated-inspections/<inspection_id>/`.

use super::super::{
    errors::{CapabilityError, CapabilityErrorCode},
    generation::repository as generation_repository,
    lifecycle,
    package_store::PackageStore,
    repository,
};
use super::comparison::{compare, CaseEvaluator, ComparisonResult};
use super::contracts::{
    InspectionContext, InspectionError, InspectionErrorCode, InspectionReceipt, InspectionReport,
    InspectionResult,
};
use super::repository as inspection_repository;
use crate::persistence::SqliteWriter;
use std::{
    fs,
    path::{Path, PathBuf},
};
#[path = "service/inspection_store.rs"]
mod inspection_store;
pub use inspection_store::{
    InspectionService, InspectionStore, Inspector, INSPECTION_DIRECTORY, REPORT_FILE,
    TYPESCRIPT_FILE,
};
#[cfg(test)]
#[path = "service/tests.rs"]
mod tests;
