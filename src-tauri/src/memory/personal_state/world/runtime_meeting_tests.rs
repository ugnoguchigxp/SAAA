#![cfg(test)]

//! M2-07 / M2-08 / M2-22: meeting owner snapshot and live-state integration.

use super::runtime_frame::MeetingReader;
use super::runtime_test_support::*;
use crate::meeting::{MeetingRuntime, MeetingState};
use saaa_personal_state_core::world::runtime_frame::{
    FrameNoticeCode, FrameValidity, RuntimeFocusReason, RuntimePhase,
};

#[test]
fn m2_07_world_snapshot_omits_token_entries_and_error() {
    let runtime = MeetingRuntime::new();
    runtime.set_test_world_state(Some("m1"), MeetingState::Active);
    let snapshot = runtime.world_snapshot().unwrap();
    assert_eq!(snapshot.session_id.as_deref(), Some("m1"));
    assert_eq!(snapshot.state, MeetingState::Active);
    // The full owner snapshot still carries the token; the World view must not.
    let full = runtime.snapshot().unwrap();
    assert!(full.capture_token.is_some());
    let text = format!("{snapshot:?}");
    assert!(!text.contains("capture_token_must_not_leak"));
    assert!(!text.contains("transcript_must_not_leak"));
    assert!(!text.contains("error_must_not_leak"));
    // The owner state is unchanged by the read.
    let full_after = runtime.snapshot().unwrap();
    assert_eq!(full_after.session_id, full.session_id);
    assert_eq!(full_after.state, full.state);
    assert_eq!(full_after.capture_token, full.capture_token);
}

#[test]
fn m2_07_default_meeting_reader_returns_idle() {
    let reader = FakeMeetingReader::new();
    let snapshot = reader.world_snapshot().unwrap();
    assert!(snapshot.session_id.is_none());
    assert_eq!(snapshot.state, MeetingState::Idle);
}

#[test]
fn m2_08_missing_row_is_runtime_unavailable() {
    let fixture = Fixture::new(&[("resource", MEETING_ID)]);
    fixture.meeting.set(Some(MEETING_ID), MeetingState::Active);
    let access = fixture.access();
    let request = fixture.request(access, vec![fixture.meeting_ref(MEETING_ID)], None);
    let frame = fixture
        .service()
        .prepare_frame(request)
        .unwrap()
        .frame()
        .clone();
    assert!(frame.runtime.is_empty());
    assert!(frame
        .notices
        .iter()
        .any(|n| n.code == FrameNoticeCode::RuntimeUnavailable));
}

#[test]
fn m2_08_discarded_row_is_runtime_unavailable() {
    let fixture = Fixture::new(&[("resource", MEETING_ID)]);
    fixture.add_meeting(MEETING_ID, "discarded", "100", None, None);
    fixture.meeting.set(Some(MEETING_ID), MeetingState::Active);
    let access = fixture.access();
    let request = fixture.request(access, vec![fixture.meeting_ref(MEETING_ID)], None);
    let frame = fixture
        .service()
        .prepare_frame(request)
        .unwrap()
        .frame()
        .clone();
    assert!(frame.runtime.is_empty());
    assert!(frame
        .notices
        .iter()
        .any(|n| n.code == FrameNoticeCode::RuntimeUnavailable));
}

#[test]
fn m2_08_malformed_time_is_owner_corrupt() {
    let fixture = Fixture::new(&[("resource", MEETING_ID)]);
    fixture.add_meeting(MEETING_ID, "active", "not-a-time", None, None);
    fixture.meeting.set(Some(MEETING_ID), MeetingState::Active);
    let access = fixture.access();
    let request = fixture.request(access, vec![fixture.meeting_ref(MEETING_ID)], None);
    assert_eq!(
        fixture.service().prepare_frame(request).unwrap_err().code(),
        "frame-owner-corrupt"
    );
}

#[test]
fn m2_22_active_meeting_yields_running_and_active_project() {
    let fixture = Fixture::new(&[("resource", MEETING_ID)]);
    fixture.add_meeting(MEETING_ID, "active", "100", None, None);
    fixture.meeting.set(Some(MEETING_ID), MeetingState::Active);
    let access = fixture.access();
    let request = fixture.request(access, vec![fixture.meeting_ref(MEETING_ID)], None);
    let frame = fixture
        .service()
        .prepare_frame(request)
        .unwrap()
        .frame()
        .clone();
    assert_eq!(frame.runtime.len(), 1);
    assert_eq!(frame.runtime[0].phase, RuntimePhase::Running);
    assert_eq!(frame.runtime_focus.len(), 1);
    assert_eq!(
        frame.runtime_focus[0].reason,
        RuntimeFocusReason::ActiveProject
    );
}

#[test]
fn m2_22_pause_makes_the_old_frame_changed() {
    let fixture = Fixture::new(&[("resource", MEETING_ID)]);
    fixture.add_meeting(MEETING_ID, "active", "100", None, None);
    fixture.meeting.set(Some(MEETING_ID), MeetingState::Active);
    let service = fixture.service();
    let access = fixture.access();
    let request = fixture.request(access, vec![fixture.meeting_ref(MEETING_ID)], None);
    let prepared = service.prepare_frame(request).unwrap();
    assert_eq!(
        service.revalidate_frame(&prepared).unwrap(),
        FrameValidity::Current
    );
    // pause -> DB and live both paused
    fixture
        .writer
        .write(|c| {
            c.execute(
                "UPDATE meeting_sessions SET status='paused' WHERE id=?1",
                [MEETING_ID],
            )
            .map_err(crate::database_error)?;
            Ok(())
        })
        .unwrap();
    fixture.meeting.set(Some(MEETING_ID), MeetingState::Paused);
    assert_eq!(
        service.revalidate_frame(&prepared).unwrap(),
        FrameValidity::Changed
    );
}

#[test]
fn m2_22_unstable_live_state_omits_the_unit() {
    let fixture = Fixture::new(&[("resource", MEETING_ID)]);
    fixture.add_meeting(MEETING_ID, "active", "100", None, None);
    // DB says active but the live owner is idle: no evidence of progress.
    fixture.meeting.set(Some(MEETING_ID), MeetingState::Idle);
    let access = fixture.access();
    let request = fixture.request(access, vec![fixture.meeting_ref(MEETING_ID)], None);
    let frame = fixture
        .service()
        .prepare_frame(request)
        .unwrap()
        .frame()
        .clone();
    assert!(frame.runtime.is_empty());
    assert!(frame.runtime_focus.is_empty());
    assert!(frame
        .notices
        .iter()
        .any(|n| n.code == FrameNoticeCode::RuntimeUnstable));
}

#[test]
fn m2_22_terminal_meeting_has_no_active_focus() {
    let fixture = Fixture::new(&[("resource", MEETING_ID)]);
    fixture.add_meeting(MEETING_ID, "saved", "100", Some("150"), Some("160"));
    fixture.meeting.clear();
    let access = fixture.access();
    let request = fixture.request(access, vec![fixture.meeting_ref(MEETING_ID)], None);
    let frame = fixture
        .service()
        .prepare_frame(request)
        .unwrap()
        .frame()
        .clone();
    assert_eq!(frame.runtime.len(), 1);
    assert_eq!(frame.runtime[0].phase, RuntimePhase::Terminal);
    assert!(frame.runtime_focus.is_empty());
}

#[test]
fn m2_22_restart_active_row_without_live_is_not_running() {
    // The World reader must use the reconciled owner state, not resurrect it.
    let fixture = Fixture::new(&[("resource", MEETING_ID)]);
    fixture.add_meeting(MEETING_ID, "active", "100", None, None);
    fixture.meeting.clear();
    let access = fixture.access();
    let request = fixture.request(access, vec![fixture.meeting_ref(MEETING_ID)], None);
    let frame = fixture
        .service()
        .prepare_frame(request)
        .unwrap()
        .frame()
        .clone();
    assert!(frame.runtime.is_empty() || frame.runtime[0].phase != RuntimePhase::Running);
    assert!(frame.runtime_focus.is_empty());
}
