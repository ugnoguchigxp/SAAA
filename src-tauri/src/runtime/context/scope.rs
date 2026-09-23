use crate::{database_error, now_iso, StartTurnInput};
use rusqlite::{params, Connection, OptionalExtension};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
#[path = "scope/resolved_scope.rs"]
mod resolved_scope;
pub(crate) use resolved_scope::{ResolvedScope, ScopeSnapshot, resolve, load, attach_output, register, link, revoke};
#[cfg(test)]
#[path = "scope/tests.rs"]
mod tests;
