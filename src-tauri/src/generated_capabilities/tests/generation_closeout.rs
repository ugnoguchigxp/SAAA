use super::generation_flow::{context, registered};
use super::{FixturePackager, GenerationService, SequencePackager};
use crate::generated_capabilities::contracts::{CallActor, InvokeRequest};
use crate::generated_capabilities::errors::CapabilityErrorCode;
use crate::generated_capabilities::generation::contracts::{
    GenerateInput, GenerationErrorCode, GenerationStatus,
};
use crate::generated_capabilities::generation::generator::{FakeBody, FakeGenerator, Generator};
use crate::generated_capabilities::host::process::Cancellation;
use crate::generated_capabilities::inspection::comparison::CaseEvaluator;
use crate::generated_capabilities::inspection::contracts::{
    tests as inspection_fixtures, InspectionContext, InspectionReport,
};
use crate::generated_capabilities::inspection::repository::{
    self as inspection_repository, NewInspection,
};
use crate::generated_capabilities::inspection::service::{
    InspectionService, InspectionStore, Inspector,
};
use crate::generated_capabilities::lifecycle;
use crate::generated_capabilities::tests::{
    candidate_dir, object, TestEnv, ACCEPTANCE_A, ACCEPTANCE_B, CANDIDATE_A, CANDIDATE_B,
};
use crate::runtime::capability_commands::load_stored_inspection;
use crate::RunCancellation;
use async_trait::async_trait;
use serde_json::{json, Map, Value};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
#[path = "generation_closeout/fixture_inspector.rs"]
mod fixture_inspector;
#[path = "generation_closeout/gc_06_epoch_change_during_update_conflicts.rs"]
mod gc_06_epoch_change_during_update_conflicts;
use fixture_inspector::{
    actor, bind_typescript, capability_id, catalog_enabled, generate_a, invoke_on, GatedGenerator,
};
pub(crate) use gc_06_epoch_change_during_update_conflicts::*;
