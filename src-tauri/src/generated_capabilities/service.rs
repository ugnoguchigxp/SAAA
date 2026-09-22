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
#[cfg(test)]
mod test_support;
pub use super::contracts::{InvocationResult, InvokeRequest, ResolvedCapability};
include!("service.d/01.rs");
include!("service.d/02.rs");
