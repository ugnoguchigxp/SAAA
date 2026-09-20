#![cfg(test)]

//! M2-10 / M2-11 / M2-23: coding owner snapshot, source eligibility and state
//! change detection.

use super::runtime_test_support::*;
use crate::coding::world_snapshot::read_world_snapshot;
use rusqlite::params;
use saaa_personal_state_core::world::runtime_frame::{
    CodingOwnerState, FrameNoticeCode, FrameValidity, RuntimeOwnerState, RuntimePhase,
};

fn extra_runs(fixture: &Fixture, start: usize, count: usize) {
    fixture
        .writer
        .write(|c| {
            for index in start..(start + count) {
                let source = format!("extra{index}");
                c.execute(
                    "INSERT INTO conversation_messages(id,conversation_id,role,content,created_at)
                     VALUES(?1,?2,'user','extra','1000')",
                    params![source, crate::PRIMARY_CONVERSATION_ID],
                )
                .map_err(crate::database_error)?;
                c.execute(
                    "INSERT INTO coding_runs(
                       id,job_id,source_id,host_run_id,payload,digest,delivery,state,started_at)
                     VALUES(?1,?2,?3,'h','{}','d','accepted','settled','1000')",
                    params![format!("extra_run{index}"), CODING_ID, source],
                )
                .map_err(crate::database_error)?;
            }
            Ok(())
        })
        .unwrap();
}

#[test]
fn m2_10_snapshot_reads_minimal_state() {
    let fixture = Fixture::new(&[("task", CODING_ID)]);
    fixture.add_coding_job_with_result(
        7,
        "running",
        "running",
        "accepted",
        Some(r#"{"complete":true}"#),
    );
    let snapshot = fixture
        .writer
        .read_serialized(|c| read_world_snapshot(c, crate::PRIMARY_CONVERSATION_ID, CODING_ID))
        .unwrap();
    assert_eq!(snapshot.revision, 7);
    assert_eq!(snapshot.job_state, "running");
    assert_eq!(snapshot.run_state, "running");
    assert_eq!(snapshot.delivery, "accepted");
    assert_eq!(snapshot.reported_complete, Some(true));
    assert_eq!(snapshot.source_ids, vec![CODING_SOURCE.to_string()]);
}

#[test]
fn m2_10_non_boolean_complete_is_null() {
    let fixture = Fixture::new(&[("task", CODING_ID)]);
    fixture.add_coding_job_with_result(
        7,
        "settled",
        "settled",
        "accepted",
        Some(r#"{"complete":"yes"}"#),
    );
    let snapshot = fixture
        .writer
        .read_serialized(|c| read_world_snapshot(c, crate::PRIMARY_CONVERSATION_ID, CODING_ID))
        .unwrap();
    assert_eq!(snapshot.reported_complete, None);
}

#[test]
fn m2_10_thirty_two_runs_are_accepted_and_thirty_three_omitted() {
    let fixture = Fixture::new(&[("task", CODING_ID)]);
    fixture.add_coding_job(1, "settled", "settled", "accepted");
    extra_runs(&fixture, 0, 31);
    let accepted = fixture
        .writer
        .read_serialized(|c| read_world_snapshot(c, crate::PRIMARY_CONVERSATION_ID, CODING_ID));
    assert!(accepted.is_ok(), "32 runs accepted");
    extra_runs(&fixture, 31, 1);
    let omitted = fixture
        .writer
        .read_serialized(|c| read_world_snapshot(c, crate::PRIMARY_CONVERSATION_ID, CODING_ID));
    assert_eq!(omitted, Err("runtime_capacity_omitted".to_string()));
}

#[test]
fn m2_11_tombstoned_source_is_unavailable() {
    let fixture = Fixture::new(&[("task", CODING_ID)]);
    fixture.add_coding_job(7, "running", "running", "accepted");
    fixture
        .writer
        .write(|c| {
            c.execute(
                "INSERT INTO personal_tombstones(source_id,forgotten_at) VALUES(?1,1)",
                [CODING_SOURCE],
            )
            .map_err(crate::database_error)?;
            Ok(())
        })
        .unwrap();
    let access = fixture.access();
    let request = fixture.request(access, vec![fixture.coding_ref(CODING_ID)], None);
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
fn m2_11_unavailable_source_is_unavailable() {
    let fixture = Fixture::new(&[("task", CODING_ID)]);
    fixture.add_coding_job(7, "running", "running", "accepted");
    fixture
        .writer
        .write(|c| {
            c.execute(
                "UPDATE personal_sources SET available=0 WHERE message_id=?1",
                [CODING_SOURCE],
            )
            .map_err(crate::database_error)?;
            Ok(())
        })
        .unwrap();
    let access = fixture.access();
    let request = fixture.request(access, vec![fixture.coding_ref(CODING_ID)], None);
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
fn m2_11_unmapped_source_is_unavailable() {
    let fixture = Fixture::new(&[("task", CODING_ID)]);
    fixture.add_coding_job(7, "running", "running", "accepted");
    fixture
        .writer
        .write(|c| {
            c.execute(
                "DELETE FROM personal_source_scope_refs WHERE source_id=?1",
                [CODING_SOURCE],
            )
            .map_err(crate::database_error)?;
            Ok(())
        })
        .unwrap();
    let access = fixture.access();
    let request = fixture.request(access, vec![fixture.coding_ref(CODING_ID)], None);
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
fn m2_23_revision_unchanged_state_change_is_detected() {
    let fixture = Fixture::new(&[("task", CODING_ID)]);
    fixture.add_coding_job(7, "running", "running", "accepted");
    let service = fixture.service();
    let access = fixture.access();
    let request = fixture.request(access, vec![fixture.coding_ref(CODING_ID)], None);
    let prepared = service.prepare_frame(request).unwrap();
    assert_eq!(prepared.frame().runtime.len(), 1);
    assert_eq!(prepared.frame().runtime[0].phase, RuntimePhase::Running);
    assert_eq!(prepared.frame().runtime_focus.len(), 1);

    fixture.set_coding_state(7, "cancel_requested", "stopping", "accepted");
    assert_eq!(
        service.revalidate_frame(&prepared).unwrap(),
        FrameValidity::Changed
    );
    let access = fixture.access();
    let request = fixture.request(access, vec![fixture.coding_ref(CODING_ID)], None);
    let frame = service.prepare_frame(request).unwrap().frame().clone();
    assert_eq!(frame.runtime[0].phase, RuntimePhase::Stopping);
    // Stopping is a request to stop, never "cancelled" / terminal.
    assert_eq!(
        frame.runtime[0].owner_state,
        RuntimeOwnerState::CodingJob(CodingOwnerState::CancelRequested)
    );
    assert!(frame.runtime_focus.is_empty());
}

#[test]
fn m2_23_settled_does_not_claim_success() {
    let fixture = Fixture::new(&[("task", CODING_ID)]);
    fixture.add_coding_job_with_result(
        7,
        "settled",
        "settled",
        "accepted",
        Some(r#"{"complete":false}"#),
    );
    let access = fixture.access();
    let request = fixture.request(access, vec![fixture.coding_ref(CODING_ID)], None);
    let frame = fixture
        .service()
        .prepare_frame(request)
        .unwrap()
        .frame()
        .clone();
    assert_eq!(frame.runtime.len(), 1);
    assert_eq!(frame.runtime[0].phase, RuntimePhase::Terminal);
    assert_eq!(frame.runtime[0].reported_complete, Some(false));
    assert!(frame.runtime_focus.is_empty());
}

#[test]
fn m2_23_outcome_unknown_is_unknown_not_terminal() {
    let fixture = Fixture::new(&[("task", CODING_ID)]);
    fixture.add_coding_job(7, "outcome_unknown", "outcome_unknown", "unknown");
    let access = fixture.access();
    let request = fixture.request(access, vec![fixture.coding_ref(CODING_ID)], None);
    let frame = fixture
        .service()
        .prepare_frame(request)
        .unwrap()
        .frame()
        .clone();
    assert_eq!(frame.runtime.len(), 1);
    assert_eq!(frame.runtime[0].phase, RuntimePhase::Unknown);
    assert!(frame.runtime_focus.is_empty());
}

#[test]
fn m2_23_unknown_state_is_omitted_not_remapped() {
    let fixture = Fixture::new(&[("task", CODING_ID)]);
    fixture.add_coding_job(7, "mystery_state", "running", "accepted");
    let access = fixture.access();
    let request = fixture.request(access, vec![fixture.coding_ref(CODING_ID)], None);
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
        .any(|n| n.code == FrameNoticeCode::RuntimeUnsupportedState));
}

#[test]
fn m2_11_unmapped_initial_source_is_unavailable() {
    let fixture = Fixture::new(&[("task", CODING_ID)]);
    fixture.add_coding_job(7, "running", "running", "accepted");
    // The job's own initial source differs from the run source and is not mapped
    // to the requested Project, so the whole job is omitted.
    fixture
        .writer
        .write(|c| {
            c.execute(
                "INSERT INTO conversation_messages(id,conversation_id,role,content,created_at)
                 VALUES('unmapped_src',?1,'user','x','1000')",
                [crate::PRIMARY_CONVERSATION_ID],
            )
            .map_err(crate::database_error)?;
            c.execute(
                "UPDATE coding_jobs SET source_id='unmapped_src' WHERE id=?1",
                [CODING_ID],
            )
            .map_err(crate::database_error)?;
            Ok(())
        })
        .unwrap();
    let access = fixture.access();
    let request = fixture.request(access, vec![fixture.coding_ref(CODING_ID)], None);
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
fn m2_10_frame_json_has_no_coding_content_or_paths() {
    let fixture = Fixture::new(&[("task", CODING_ID)]);
    fixture.add_coding_job_with_result(
        7,
        "running",
        "running",
        "accepted",
        Some(r#"{"complete":true,"secret":"do-not-leak"}"#),
    );
    let access = fixture.access();
    let request = fixture.request(access, vec![fixture.coding_ref(CODING_ID)], None);
    let frame = fixture
        .service()
        .prepare_frame(request)
        .unwrap()
        .frame()
        .clone();
    let text = serde_json::to_string(&frame).unwrap();
    assert!(!text.contains("do-not-leak"));
    assert!(!text.contains("/tmp/w"));
    assert!(!text.contains("/tmp/session"));
    assert!(!text.contains("host"));
    assert!(!text.contains("payload"));
    assert_eq!(frame.runtime[0].reported_complete, Some(true));
}
