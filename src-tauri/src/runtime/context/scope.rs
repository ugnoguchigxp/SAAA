use crate::{database_error, now_iso, StartTurnInput};
use rusqlite::{params, Connection, OptionalExtension};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
#[path = "scope/resolved_scope.rs"]
mod resolved_scope;
pub(crate) use resolved_scope::{
    attach_output, link, load, register, resolve, revoke, ResolvedScope, ScopeSnapshot,
};
#[cfg(test)]
#[path = "scope/tests.rs"]
mod tests;
