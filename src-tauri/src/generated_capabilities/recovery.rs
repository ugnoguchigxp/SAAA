use super::{
    errors::*, lifecycle, package_store::PackageStore, repository, service::CapabilityService,
};
use crate::now_iso;

#[derive(Clone, Debug, Default)]
pub struct RecoverySummary {
    pub interrupted_checks: usize,
    pub interrupted_calls: usize,
    pub interrupted_imports: usize,
    pub inconsistent_capabilities: Vec<String>,
    pub missing_packages: Vec<String>,
    pub orphan_packages: Vec<String>,
}

/// Startup reconciliation. It only inspects the database and the managed file area; it never
/// runs a candidate and never retries a verification or an import.
pub fn reconcile_startup(service: &CapabilityService) -> CapabilityResult<RecoverySummary> {
    let (interrupted_checks, interrupted_calls, interrupted_imports, inconsistent) =
        lifecycle::transaction(service.writer(), |transaction| {
            let (checks, calls, imports) = repository::interrupt_running(transaction)?;
            let inconsistent = repository::stop_inconsistent_capabilities(transaction, &now_iso())?;
            Ok((checks, calls, imports, inconsistent))
        })?;

    let active = service
        .writer()
        .read_serialized(|connection| {
            repository::active_revisions(connection).map_err(|error| error.encode())
        })
        .map_err(CapabilityError::decode)?;

    let mut missing_packages = Vec::new();
    for revision in &active {
        let matched = service
            .store()
            .package_inventory(&revision.package_hash)
            .map(|inventory| PackageStore::inventory_hash(&inventory) == revision.inventory_hash)
            .unwrap_or(false);
        if !matched {
            missing_packages.push(revision.package_hash.clone());
            lifecycle::transaction(service.writer(), |transaction| {
                repository::stop_capability(transaction, &revision.capability_id, &now_iso())
            })?;
        }
    }

    let known = service
        .writer()
        .read_serialized(|connection| {
            repository::package_hashes_in_use(connection).map_err(|error| error.encode())
        })
        .map_err(CapabilityError::decode)?;
    let orphan_packages = service.store().orphan_packages(&known)?;

    let mut inconsistent_capabilities = inconsistent;
    inconsistent_capabilities.sort();
    inconsistent_capabilities.dedup();
    missing_packages.sort();
    missing_packages.dedup();

    Ok(RecoverySummary {
        interrupted_checks,
        interrupted_calls,
        interrupted_imports,
        inconsistent_capabilities,
        missing_packages,
        orphan_packages,
    })
}
