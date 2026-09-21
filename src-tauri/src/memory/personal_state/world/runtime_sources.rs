//! Reads source-owned metadata inside the FrameService's read transaction.
use super::runtime_scope::AuthorizedFrame;
use rusqlite::{params, Connection};
use saaa_personal_state_core::world::{
    frame_sources::*,
    runtime_frame::{hex_sha256, FrameError},
};
use serde_json::json;

fn failure(error: rusqlite::Error) -> FrameError {
    FrameError::Other(crate::database_error(error))
}
fn entry(
    kind: WorldSourceKind,
    id: String,
    scope: String,
    version: String,
    payload: WorldSourcePayload,
    now: i64,
) -> WorldSourceEntry {
    let digest = hex_sha256(
        serde_json::to_vec(&json!([&id, &scope, &version, &payload]))
            .expect("source encodes")
            .as_slice(),
    );
    WorldSourceEntry {
        kind,
        source_id: id,
        owner_scope_key: scope,
        availability: WorldSourceAvailability::Available,
        observed_at_ms: now,
        as_of_ms: now,
        version: Some(version),
        digest,
        payload: Some(payload),
        reason_code: None,
    }
}
fn group(kind: WorldSourceKind, entries: Vec<WorldSourceEntry>) -> WorldSourceGroup {
    WorldSourceGroup {
        kind,
        availability: WorldSourceAvailability::Available,
        entries,
        omission_reason: None,
    }
}
pub(super) fn read(
    c: &Connection,
    auth: &AuthorizedFrame,
    now: i64,
) -> Result<Vec<WorldSourceGroup>, FrameError> {
    let keys = &auth.scope.allowed_scope_keys;
    let mut coding = Vec::new();
    for target in &auth.targets {
        let trusted = super::runtime_coding::read_view(c, auth, &target.reference)?;
        if trusted.unit.is_none() {
            coding.push(WorldSourceEntry {
                kind: WorldSourceKind::Coding,
                source_id: target.reference.id.clone(),
                owner_scope_key: format!("task:{}", target.reference.id),
                availability: WorldSourceAvailability::Unavailable,
                observed_at_ms: now,
                as_of_ms: now,
                version: None,
                digest: hex_sha256(format!("{}:unavailable", target.reference.id).as_bytes()),
                payload: None,
                reason_code: Some("owner-or-source-unavailable".into()),
            });
            continue;
        }
        let row = c
            .query_row(
                "SELECT state,revision FROM coding_jobs WHERE id=?1",
                [&target.reference.id],
                |r| Ok((r.get::<_, String>(0)?, r.get::<_, u64>(1)?)),
            )
            .map_err(failure)?;
        coding.push(entry(
            WorldSourceKind::Coding,
            target.reference.id.clone(),
            format!("task:{}", target.reference.id),
            row.1.to_string(),
            WorldSourcePayload::Coding {
                job_id: target.reference.id.clone(),
                owner_state: row.0.clone(),
                phase: match row.0.as_str() {
                    "queued" => "starting",
                    "running" => "running",
                    "cancel_requested" => "stopping",
                    "settled" | "failed" | "interrupted" => "terminal",
                    _ => "unknown",
                }
                .into(),
                revision: Some(row.1),
            },
            now,
        ));
    }
    let mut deadlines = Vec::new();
    let mut delegated = Vec::new();
    // user-only state does not gain permission to read task or schedule data.
    for key in keys.iter().filter(|key| !key.starts_with("user:")) {
        let mut q = c.prepare("SELECT id,status,due_at,revision FROM schedule_entries WHERE scope_ref=?1 AND status IN ('scheduled','firing') ORDER BY due_at,id LIMIT 8").map_err(failure)?;
        let rows = q
            .query_map([key], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, i64>(2)?,
                    r.get::<_, u64>(3)?,
                ))
            })
            .map_err(failure)?;
        for row in rows {
            let (id, status, due, revision) = row.map_err(failure)?;
            deadlines.push((
                due,
                entry(
                    WorldSourceKind::Schedule,
                    id.clone(),
                    key.clone(),
                    revision.to_string(),
                    WorldSourcePayload::Schedule {
                        entry_id: id,
                        status,
                        due_at_ms: Some(due),
                        revision: Some(revision),
                    },
                    now,
                ),
            ));
        }
        let Some(workspace) = key.strip_prefix("resource:") else {
            continue;
        };
        let mut q = c.prepare("SELECT t.id,t.loop_state,t.revision,d.id,d.status,d.revision,g.id,g.status,g.revision,t.coding_job_id
            FROM steward_tasks t JOIN steward_delegations d ON d.id=t.delegation_id JOIN steward_goals g ON g.id=d.goal_id
            WHERE d.workspace_id=?1 AND t.conversation_id=?2 AND d.conversation_id=?2 AND g.conversation_id=?2
            ORDER BY t.updated_at DESC,t.id LIMIT 8").map_err(failure)?;
        let rows = q
            .query_map(params![workspace, auth.conversation_id], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, u64>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, String>(4)?,
                    r.get::<_, u64>(5)?,
                    r.get::<_, String>(6)?,
                    r.get::<_, String>(7)?,
                    r.get::<_, u64>(8)?,
                    r.get::<_, Option<String>>(9)?,
                ))
            })
            .map_err(failure)?;
        for row in rows {
            let (
                id,
                status,
                revision,
                delegation_id,
                delegation_status,
                dr,
                goal_id,
                goal_status,
                gr,
                coding_job,
            ) = row.map_err(failure)?;
            if coding_job
                .as_ref()
                .is_some_and(|id| coding.iter().any(|entry| &entry.source_id == id))
            {
                continue;
            }
            delegated.push(entry(
                WorldSourceKind::Delegation,
                id.clone(),
                key.clone(),
                format!("{revision}/{dr}/{gr}"),
                WorldSourcePayload::Delegation {
                    task_id: id,
                    delegation_id: Some(delegation_id),
                    goal_id: Some(goal_id),
                    status: status.clone(),
                    loop_state: Some(status),
                    revision: Some(revision),
                    delegation_status,
                    goal_status,
                },
                now,
            ));
        }
    }
    deadlines.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.source_id.cmp(&b.1.source_id)));
    deadlines.dedup_by(|a, b| a.1.source_id == b.1.source_id);
    deadlines.truncate(8);
    delegated.sort_by(|a, b| a.source_id.cmp(&b.source_id));
    delegated.dedup_by(|a, b| a.source_id == b.source_id);
    delegated.truncate(8usize.saturating_sub(coding.len()));
    if keys.iter().all(|key| key.starts_with("user:")) {
        return Ok([
            WorldSourceKind::Coding,
            WorldSourceKind::Delegation,
            WorldSourceKind::Schedule,
        ]
        .into_iter()
        .map(|kind| WorldSourceGroup {
            kind,
            availability: WorldSourceAvailability::Unavailable,
            entries: vec![],
            omission_reason: Some("scope-not-selected".into()),
        })
        .collect());
    }
    Ok(vec![
        group(WorldSourceKind::Coding, coding),
        group(WorldSourceKind::Delegation, delegated),
        group(
            WorldSourceKind::Schedule,
            deadlines.into_iter().map(|(_, e)| e).collect(),
        ),
    ])
}
