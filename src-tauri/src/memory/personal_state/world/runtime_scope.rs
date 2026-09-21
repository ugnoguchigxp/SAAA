#![allow(dead_code)]
//! Trusted request authorization for the M2A WorldFrame (R2). Read-only: the
//! resolver (`scope::resolve`) is never called and nothing is registered or
//! linked from this path.

use super::runtime_frame::FrameRequest;
use crate::database_error;
use crate::runtime::context::scope;
use rusqlite::{params, Connection};
use saaa_personal_state_core::world::runtime_frame::{
    runtime_scope_key, FrameError, RuntimeKind, RuntimeRef,
};
use saaa_personal_state_core::{Classification, Purpose};
use sha2::{Digest, Sha256};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AuthorizedTarget {
    pub(crate) reference: RuntimeRef,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AuthorizedFrame {
    pub(crate) run_id: String,
    pub(crate) project_scope: String,
    pub(crate) conversation_id: String,
    pub(crate) ledger_revision: u64,
    pub(crate) input_epoch: u64,
    pub(crate) policy_revision: u64,
    pub(crate) scope_digest: String,
    pub(crate) targets: Vec<AuthorizedTarget>,
    pub(crate) scope: saaa_personal_state_core::world::frame_sources::WorldScope,
}

struct Header {
    principal: String,
    ledger_revision: u64,
    input_epoch: u64,
    policy_revision: u64,
}

fn load_header(c: &Connection) -> Result<Header, FrameError> {
    c.query_row(
        "SELECT principal,revision,input_epoch,policy_revision FROM personal_scope WHERE id='primary'",
        [],
        |row| {
            Ok(Header {
                principal: row.get(0)?,
                ledger_revision: row.get(1)?,
                input_epoch: row.get(2)?,
                policy_revision: row.get(3)?,
            })
        },
    )
    .map_err(|_| FrameError::ScopeDenied)
}

fn active_epoch(c: &Connection, scope_key: &str) -> Result<Option<u64>, FrameError> {
    let result: Option<u64> = c
        .query_row(
            "SELECT e.epoch FROM context_scope_epochs e
             JOIN context_scopes s ON s.scope_key=e.scope_key
             WHERE s.scope_key=?1 AND s.state='active'",
            [scope_key],
            |row| row.get(0),
        )
        .ok();
    Ok(result)
}

fn direct_link(c: &Connection, project: &str, target: &str) -> Result<bool, FrameError> {
    c.query_row(
        "SELECT EXISTS(SELECT 1 FROM context_scope_links
         WHERE parent_scope_key=?1 AND child_scope_key=?2 AND relation IN ('parent','owns'))",
        params![project, target],
        |row| row.get(0),
    )
    .map_err(|e| FrameError::Other(database_error(e)))
}

fn scope_kind(c: &Connection, scope_key: &str) -> Result<(String, String), FrameError> {
    c.query_row(
        "SELECT kind,opaque_id FROM context_scopes WHERE scope_key=?1",
        [scope_key],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )
    .map_err(|_| FrameError::ScopeDenied)
}

fn hash(values: &[serde_json::Value]) -> String {
    let text = serde_json::to_string(&values.to_vec()).unwrap_or_default();
    format!("{:x}", Sha256::digest(text.as_bytes()))
}

#[path = "runtime_scope_authorize.rs"]
mod authorize;
pub(crate) use authorize::authorize_frame_request;
