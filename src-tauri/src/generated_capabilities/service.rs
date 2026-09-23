pub use super::contracts::{InvocationResult, InvokeRequest, ResolvedCapability};
use super::{
    contracts::{self, HostOutcome, HostRequest, OperationResult, PackageManifest, WasmContract},
    errors::*,
    execution::{self, ExecutionGuard, ExecutionKind, ExecutionRegistry, HostInvocation},
    guards::{activate_revision, finalize_check, validate_input, CheckOutcome},
    host::{
        process::{self, Cancellation},
        runtime_bundle::{self, RuntimeConfig},
        WasmHost,
    },
    lifecycle, limits,
    package_store::{PackageStore, StagedPackage},
    repository::{self, RevisionState},
    verification::{self, AcceptanceLedger},
};
use crate::{now_iso, persistence::SqliteWriter};
use serde_json::Value;
use std::{
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};
use tokio::sync::{oneshot, Mutex as AsyncMutex, OwnedSemaphorePermit, Semaphore};
#[path = "service/import_candidate.rs"]
mod import_candidate;
#[cfg(test)]
#[path = "service/test_support.rs"]
mod test_support;
#[path = "service/verify_inner.rs"]
mod verify_inner;
pub use import_candidate::{CapabilityService, ImportCandidate, RevisionRef, VerificationSummary};
pub(super) use verify_inner::storage_error;
// Methods on CapabilityService live in child impl blocks.
