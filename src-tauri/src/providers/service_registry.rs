//! Purpose-based service registry (plan: docs/plans/purpose-based-cloud-api-switching.md, P1).
//!
//! Three layers: connections (where and how to authenticate), resources (what a
//! connection can run) and purpose bindings (which resource does which job).
//! This module is pure data, validation, legacy migration and route resolution.
//! It is not yet persisted or consulted by the conversation queue.
pub(crate) mod commands;
mod migration;
mod resolve;
#[cfg(test)]
mod tests;
mod types;
mod validate;

pub(crate) use migration::migrate_legacy;
pub(crate) const SERVICE_CREDENTIAL_SERVICE: &str =
    crate::credentials::SERVICE_CONNECTION_CREDENTIAL_SERVICE;
pub(crate) use resolve::{resolve_route, ResolveError, ResolvedRoute};
pub(crate) use types::*;
pub(crate) use validate::validate_snapshot;
