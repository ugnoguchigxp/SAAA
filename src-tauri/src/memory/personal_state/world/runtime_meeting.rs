#![allow(dead_code)]
//! Read-only meeting owner adapter for the World Frame (R3). Reads only the
//! minimal `meeting_sessions` columns and maps them together with the live
//! owner snapshot. Transcript text, capture tokens and error messages never
//! cross this boundary.

use crate::meeting::{MeetingState, WorldMeetingSnapshot};
use rusqlite::Connection;
use saaa_personal_state_core::world::runtime_frame::{
    focus_for, map_meeting, runtime_scope_key, FrameError, FrameNoticeCode, MeetingLivePhase,
    MeetingMapping, MeetingSnapshotInput, RuntimeOwnerState, RuntimeRef, RuntimeStateView,
    RuntimeUnit,
};

/// The minimal DB row needed for a meeting runtime view. Times are kept as
/// strings after parse validation so the canonical digest matches the owner.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MeetingDbRow {
    pub(crate) status: Option<String>,
    pub(crate) started_at: Option<String>,
    pub(crate) ended_at: Option<String>,
    pub(crate) saved_at: Option<String>,
}

impl MeetingDbRow {
    pub(crate) fn input<'a>(
        &'a self,
        live: Option<&'a WorldMeetingSnapshot>,
    ) -> MeetingSnapshotInput<'a> {
        let (live_session_id, live_state) = match live {
            Some(snapshot) => (
                snapshot.session_id.as_deref(),
                Some(meeting_live_phase(&snapshot.state)),
            ),
            None => (None, None),
        };
        MeetingSnapshotInput {
            db_status: self.status.as_deref(),
            started_at: self.started_at.as_deref(),
            ended_at: self.ended_at.as_deref(),
            saved_at: self.saved_at.as_deref(),
            live_session_id,
            live_state,
        }
    }
}

pub(crate) fn meeting_live_phase(state: &MeetingState) -> MeetingLivePhase {
    match state {
        MeetingState::Idle => MeetingLivePhase::Idle,
        MeetingState::Preflight => MeetingLivePhase::Preflight,
        MeetingState::Ready => MeetingLivePhase::Ready,
        MeetingState::Active => MeetingLivePhase::Active,
        MeetingState::Paused => MeetingLivePhase::Paused,
        MeetingState::Stopping => MeetingLivePhase::Stopping,
        MeetingState::Completed => MeetingLivePhase::Completed,
        MeetingState::Failed => MeetingLivePhase::Failed,
    }
}

fn parse_millis(value: &str) -> Result<(), FrameError> {
    value
        .parse::<i64>()
        .map(|_| ())
        .map_err(|_| FrameError::OwnerCorrupt)
}

/// Read `id,status,started_at,ended_at,saved_at`. A missing row is reported as
/// `status: None` so the core mapper returns `runtime_unavailable`; a malformed
/// timestamp is corruption, never an empty state.
pub(crate) fn read_row(c: &Connection, id: &str) -> Result<MeetingDbRow, FrameError> {
    let row = c
        .query_row(
            "SELECT status,started_at,ended_at,saved_at FROM meeting_sessions WHERE id=?1",
            [id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, Option<String>>(3)?,
                ))
            },
        )
        .ok();
    let Some((status, started_at, ended_at, saved_at)) = row else {
        return Ok(MeetingDbRow {
            status: None,
            started_at: None,
            ended_at: None,
            saved_at: None,
        });
    };
    parse_millis(&started_at)?;
    if let Some(value) = &ended_at {
        parse_millis(value)?;
    }
    if let Some(value) = &saved_at {
        parse_millis(value)?;
    }
    Ok(MeetingDbRow {
        status: Some(status),
        started_at: Some(started_at),
        ended_at,
        saved_at,
    })
}

pub(crate) struct MeetingRead {
    pub(crate) mapping: MeetingMapping,
    pub(crate) unit: Option<RuntimeUnit>,
    pub(crate) db: MeetingDbRow,
}

/// Map one meeting target. The DB row and the live snapshot are combined by the
/// pure core rule so the same function can be re-run with an after snapshot.
pub(crate) fn read_view(
    c: &Connection,
    project_scope: &str,
    reference: &RuntimeRef,
    live: Option<&WorldMeetingSnapshot>,
) -> Result<MeetingRead, FrameError> {
    let db = read_row(c, &reference.id)?;
    let mapping = map_meeting(&reference.id, &db.input(live));
    let unit = match &mapping {
        MeetingMapping::Present {
            owner_state,
            phase,
            digest,
        } => {
            let view = RuntimeStateView {
                reference: reference.clone(),
                scope_key: runtime_scope_key(reference),
                owner_state: RuntimeOwnerState::MeetingSession(*owner_state),
                phase: *phase,
                job_revision: None,
                current_run_id: None,
                reported_complete: None,
                owner_digest: digest.clone(),
            };
            let focus = focus_for(&view, project_scope);
            Some(RuntimeUnit { view, focus })
        }
        _ => None,
    };
    Ok(MeetingRead { mapping, unit, db })
}

/// After the DB transaction has closed, re-evaluate a meeting probe with the
/// second live snapshot. A changed mapping omits the unit as `runtime_unstable`.
pub(crate) fn recheck(
    reference: &RuntimeRef,
    db: &MeetingDbRow,
    before: &MeetingMapping,
    after: Option<&WorldMeetingSnapshot>,
) -> Option<FrameNoticeCode> {
    let after_mapping = map_meeting(&reference.id, &db.input(after));
    if &after_mapping != before {
        Some(FrameNoticeCode::RuntimeUnstable)
    } else {
        None
    }
}
