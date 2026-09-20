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

/// R2: verify the trusted request, current run context, project linkage and
/// every requested resource/task scope against the same DB snapshot. Any
/// failure denies the whole request before any owner id lookup.
pub(crate) fn authorize_frame_request(
    c: &Connection,
    request: &FrameRequest<'_>,
) -> Result<AuthorizedFrame, FrameError> {
    let access = &request.access;
    if !access.authorized
        || access.scope != "primary"
        || access.task_request != Some(request.project_scope)
        || !matches!(access.purpose, Purpose::Reasoning | Purpose::Diagnostics)
        || access.max_classification < Classification::Confidential
    {
        return Err(FrameError::ScopeDenied);
    }
    let project = request.project_scope;
    let Some(project_id) = project.strip_prefix("project:") else {
        return Err(FrameError::ScopeDenied);
    };
    if !saaa_personal_state_core::world::runtime_frame::validate_frame_identifier(project_id)
        || crate::validate_identifier(request.run_id, "run id").is_err()
    {
        return Err(FrameError::ScopeDenied);
    }
    let header = load_header(c)?;
    if header.principal != access.principal || header.policy_revision != access.policy_revision {
        return Err(FrameError::ScopeDenied);
    }
    let (conversation_id, run_status, input_message_id): (String, String, Option<String>) = c
        .query_row(
            "SELECT conversation_id,status,input_message_id FROM runtime_runs WHERE id=?1",
            [&request.run_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .map_err(|_| FrameError::ScopeDenied)?;
    if run_status != "running" {
        return Err(FrameError::ScopeDenied);
    }
    let Some(input_message_id) = input_message_id else {
        return Err(FrameError::ScopeDenied);
    };
    let (content, source_ok): (String, bool) = c
        .query_row(
            "SELECT m.content,
                    (m.conversation_id=?2 AND m.role IN ('user','transcript')
                     AND NOT EXISTS(SELECT 1 FROM personal_tombstones t WHERE t.source_id=m.id))
             FROM conversation_messages m WHERE m.id=?1",
            params![input_message_id, conversation_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(|_| FrameError::ScopeDenied)?;
    if !source_ok {
        return Err(FrameError::ScopeDenied);
    }
    let snapshot = scope::load(c, request.run_id).map_err(|_| FrameError::ScopeDenied)?;
    if snapshot.status != "resolved" {
        return Err(FrameError::ScopeDenied);
    }
    // The requested Project must be an explicit focus/parent of this run and
    // still active with a matching epoch.
    let project_scope = snapshot
        .scopes
        .iter()
        .find(|s| s.key == project && matches!(s.relation.as_str(), "focus" | "parent"))
        .ok_or(FrameError::ScopeDenied)?
        .clone();
    let project_epoch = active_epoch(c, project)?.ok_or(FrameError::ScopeDenied)?;
    if project_epoch != project_scope.epoch {
        return Err(FrameError::ScopeDenied);
    }
    if scope_kind(c, project)?.0 != "project" {
        return Err(FrameError::ScopeDenied);
    }

    let mut targets = Vec::new();
    let mut digest_parts = vec![
        serde_json::Value::from(request.run_id),
        serde_json::Value::from(input_message_id.as_str()),
        serde_json::Value::from(hash(&[serde_json::Value::from(content.as_str())])),
    ];
    // Project participation in the digest, including the direct-link check.
    digest_parts.push(serde_json::json!([
        project,
        project_scope.relation,
        project_scope.epoch,
        project_epoch,
    ]));
    for reference in &request.runtime_refs {
        let scope_key = runtime_scope_key(reference);
        let (kind, opaque_id) = scope_kind(c, &scope_key)?;
        let expected_kind = match reference.kind {
            RuntimeKind::CodingJob => "task",
        };
        if kind != expected_kind || opaque_id != reference.id {
            return Err(FrameError::ScopeDenied);
        }
        let recorded = snapshot
            .scopes
            .iter()
            .find(|s| {
                s.key == scope_key && matches!(s.relation.as_str(), "focus" | "current" | "parent")
            })
            .ok_or(FrameError::ScopeDenied)?
            .clone();
        let current = active_epoch(c, &scope_key)?.ok_or(FrameError::ScopeDenied)?;
        if current != recorded.epoch {
            return Err(FrameError::ScopeDenied);
        }
        if !direct_link(c, project, &scope_key)? {
            return Err(FrameError::ScopeDenied);
        }
        digest_parts.push(serde_json::json!([
            scope_key,
            kind,
            recorded.relation,
            recorded.epoch,
            current,
            true,
        ]));
        targets.push(AuthorizedTarget {
            reference: reference.clone(),
        });
    }
    let scope_digest = hash(&digest_parts);
    Ok(AuthorizedFrame {
        run_id: request.run_id.to_string(),
        project_scope: project.to_string(),
        conversation_id,
        ledger_revision: header.ledger_revision,
        input_epoch: header.input_epoch,
        policy_revision: header.policy_revision,
        scope_digest,
        targets,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use saaa_personal_state_core::world::runtime_frame::RuntimeRef;

    #[test]
    fn hash_is_stable_and_order_sensitive() {
        let a = hash(&[serde_json::Value::from("a"), serde_json::Value::from(1)]);
        let b = hash(&[serde_json::Value::from("a"), serde_json::Value::from(1)]);
        let c = hash(&[serde_json::Value::from(1), serde_json::Value::from("a")]);
        assert_eq!(a, b);
        assert_ne!(a, c);
    }

    #[test]
    fn runtime_scope_key_matches_kind() {
        assert_eq!(
            runtime_scope_key(&RuntimeRef {
                kind: RuntimeKind::CodingJob,
                id: "j1".into()
            }),
            "task:j1"
        );
    }
}
