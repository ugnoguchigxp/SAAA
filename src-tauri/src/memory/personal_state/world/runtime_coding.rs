#![allow(dead_code)]
//! Coding owner adapter for the World Frame (R4/R11). Reuses
//! `coding::world_snapshot::read_world_snapshot` for the bounded owner read,
//! then verifies every run source still exists, is available, is not
//! tombstoned and is mapped to the requested Project before any state is
//! reported. Source text never crosses this boundary.

use super::runtime_scope::AuthorizedFrame;
use crate::coding::world_snapshot::read_world_snapshot;
use crate::database_error;
use rusqlite::{params, Connection};
use saaa_personal_state_core::world::runtime_frame::{
    focus_for, map_coding, runtime_scope_key, CodingMapping, CodingSnapshotInput, FrameError,
    FrameNoticeCode, RuntimeOwnerState, RuntimeRef, RuntimeStateView, RuntimeUnit,
};

pub(crate) struct CodingRead {
    pub(crate) unit: Option<RuntimeUnit>,
    pub(crate) notice: Option<FrameNoticeCode>,
}

fn unavailable() -> CodingRead {
    CodingRead {
        unit: None,
        notice: Some(FrameNoticeCode::RuntimeUnavailable),
    }
}

fn notice(code: FrameNoticeCode) -> CodingRead {
    CodingRead {
        unit: None,
        notice: Some(code),
    }
}

/// Verify the initial, current and historical run sources against the current
/// Personal State. Returns `(source_id, version, project_scope)` sorted by id.
fn verify_sources(
    c: &Connection,
    conversation_id: &str,
    project_scope: &str,
    source_ids: &[String],
    initial_source_id: &str,
) -> Result<Vec<(String, u64, String)>, FrameError> {
    // The initial job source, the current run source and every historical run
    // source are all verified. The owner returns distinct run sources; the job
    // row's own source is added explicitly so a job with no run row cannot
    // bypass the check.
    let mut ids: Vec<String> = source_ids.to_vec();
    ids.push(initial_source_id.to_string());
    ids.sort();
    ids.dedup();
    let mut versions = Vec::new();
    for id in &ids {
        let current: Option<(u64, String, String)> = c
            .query_row(
                "SELECT s.version,m.role,m.conversation_id
                 FROM personal_sources s
                 JOIN conversation_messages m ON m.id=s.message_id
                 WHERE s.message_id=?1 AND s.available=1
                 ORDER BY s.version DESC LIMIT 1",
                [id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .ok();
        let Some((version, role, conversation)) = current else {
            return Err(FrameError::Unavailable);
        };
        if conversation != conversation_id || !matches!(role.as_str(), "user" | "transcript") {
            return Err(FrameError::Unavailable);
        }
        let tombstoned: bool = c
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM personal_tombstones WHERE source_id=?1)",
                [id],
                |row| row.get(0),
            )
            .map_err(|e| FrameError::Other(database_error(e)))?;
        if tombstoned {
            return Err(FrameError::Unavailable);
        }
        let mapped: bool = c
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM personal_source_scope_refs
                 WHERE source_id=?1 AND version=?2 AND scope_key=?3)",
                params![id, version, project_scope],
                |row| row.get(0),
            )
            .map_err(|e| FrameError::Other(database_error(e)))?;
        if !mapped {
            return Err(FrameError::Unavailable);
        }
        versions.push((id.clone(), version, project_scope.to_string()));
    }
    versions.sort();
    versions.dedup();
    Ok(versions)
}

pub(crate) fn read_view(
    c: &Connection,
    authorized: &AuthorizedFrame,
    reference: &RuntimeRef,
) -> Result<CodingRead, FrameError> {
    let snapshot = match read_world_snapshot(c, &authorized.conversation_id, &reference.id) {
        Ok(snapshot) => snapshot,
        Err(code) if code == "runtime_capacity_omitted" => {
            return Ok(notice(FrameNoticeCode::RuntimeCapacityOmitted))
        }
        Err(code) if code == "job_unavailable" || code == "source_unavailable" => {
            return Ok(unavailable())
        }
        Err(code) => return Err(FrameError::Other(code)),
    };
    let source_versions = match verify_sources(
        c,
        &authorized.conversation_id,
        &authorized.project_scope,
        &snapshot.source_ids,
        &snapshot.source_id,
    ) {
        Ok(versions) => versions,
        Err(FrameError::Unavailable) => return Ok(unavailable()),
        Err(error) => return Err(error),
    };
    let input = CodingSnapshotInput {
        job_id: &snapshot.job_id,
        conversation_id: &snapshot.conversation_id,
        revision: snapshot.revision,
        job_state: &snapshot.job_state,
        current_run_id: Some(&snapshot.current_run_id),
        run_state: Some(&snapshot.run_state),
        delivery: Some(&snapshot.delivery),
        ended_at: snapshot.ended_at.as_deref(),
        reported_complete: snapshot.reported_complete,
        source_versions,
    };
    match map_coding(&input) {
        CodingMapping::UnsupportedState => Ok(notice(FrameNoticeCode::RuntimeUnsupportedState)),
        CodingMapping::Present {
            owner_state,
            phase,
            digest,
        } => {
            let view = RuntimeStateView {
                reference: reference.clone(),
                scope_key: runtime_scope_key(reference),
                owner_state: RuntimeOwnerState::CodingJob(owner_state),
                phase,
                job_revision: Some(snapshot.revision),
                current_run_id: Some(snapshot.current_run_id),
                reported_complete: snapshot.reported_complete,
                owner_digest: digest,
            };
            let focus = focus_for(&view, &authorized.project_scope);
            Ok(CodingRead {
                unit: Some(RuntimeUnit { view, focus }),
                notice: None,
            })
        }
    }
}
