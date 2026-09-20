#![cfg(test)]

//! M2-05 / M2-06 / M2-21: scope authorization negatives. Every case must deny
//! the whole request before any owner id is looked up, and must not leak a name
//! or state from another project.

use super::runtime_frame::FrameRequest;
use super::runtime_test_support::*;
use saaa_personal_state_core::world::runtime_frame::{FrameError, RuntimeRef};
use saaa_personal_state_core::{AccessRequest, Classification, Purpose};

fn prepare(fixture: &Fixture, request: FrameRequest<'_>) -> Result<(), FrameError> {
    fixture.service().prepare_frame(request).map(|_| ())
}

#[test]
fn m2_05_authorized_coding_request_succeeds() {
    let fixture = Fixture::new(&[("task", CODING_ID)]);
    fixture.add_coding_job(1, "running", "running", "accepted");
    let access = fixture.access();
    let request = fixture.request(access, vec![fixture.coding_ref(CODING_ID)], None);
    assert!(prepare(&fixture, request).is_ok());
}

#[test]
fn m2_05_misclassified_access_is_denied() {
    let fixture = Fixture::new(&[("task", CODING_ID)]);
    fixture.add_coding_job(1, "running", "running", "accepted");
    let mut access = fixture.access();
    access.max_classification = Classification::Internal;
    let request = fixture.request(access, vec![fixture.coding_ref(CODING_ID)], None);
    assert_eq!(prepare(&fixture, request), Err(FrameError::ScopeDenied));
}

#[test]
fn m2_05_wrong_principal_is_denied() {
    let fixture = Fixture::new(&[("task", CODING_ID)]);
    fixture.add_coding_job(1, "running", "running", "accepted");
    let access = AccessRequest {
        principal: "someone_else",
        scope: "primary",
        task_request: Some(&fixture.project),
        purpose: Purpose::Reasoning,
        max_classification: Classification::Confidential,
        policy_revision: fixture.policy_revision,
        authorized: true,
    };
    let request = fixture.request(access, vec![fixture.coding_ref(CODING_ID)], None);
    assert_eq!(prepare(&fixture, request), Err(FrameError::ScopeDenied));
}

#[test]
fn m2_05_stale_policy_is_denied() {
    let fixture = Fixture::new(&[("task", CODING_ID)]);
    fixture.add_coding_job(1, "running", "running", "accepted");
    let mut access = fixture.access();
    access.policy_revision = fixture.policy_revision + 1;
    let request = fixture.request(access, vec![fixture.coding_ref(CODING_ID)], None);
    assert_eq!(prepare(&fixture, request), Err(FrameError::ScopeDenied));
}

#[test]
fn m2_05_finished_run_is_denied() {
    let fixture = Fixture::new(&[("task", CODING_ID)]);
    fixture.add_coding_job(1, "running", "running", "accepted");
    fixture
        .writer
        .write(|c| {
            c.execute(
                "UPDATE runtime_runs SET status='completed' WHERE id=?1",
                [RUN_ID],
            )
            .map_err(crate::database_error)?;
            Ok(())
        })
        .unwrap();
    let access = fixture.access();
    let request = fixture.request(access, vec![fixture.coding_ref(CODING_ID)], None);
    assert_eq!(prepare(&fixture, request), Err(FrameError::ScopeDenied));
}

#[test]
fn m2_06_unregistered_target_is_denied_without_leaking() {
    let fixture = Fixture::new(&[("task", CODING_ID)]);
    fixture.add_coding_job(1, "running", "running", "accepted");
    let access = fixture.access();
    let request = fixture.request(access, vec![fixture.coding_ref("other")], None);
    assert_eq!(prepare(&fixture, request), Err(FrameError::ScopeDenied));
}

#[test]
fn m2_06_removed_direct_link_is_denied_without_epoch_change() {
    let fixture = Fixture::new(&[("task", CODING_ID)]);
    fixture.add_coding_job(1, "running", "running", "accepted");
    fixture
        .writer
        .write(|c| {
            c.execute(
                "DELETE FROM context_scope_links WHERE parent_scope_key=?1 AND child_scope_key=?2",
                rusqlite::params![&fixture.project, "task:j1"],
            )
            .map_err(crate::database_error)?;
            Ok(())
        })
        .unwrap();
    let access = fixture.access();
    let request = fixture.request(access, vec![fixture.coding_ref(CODING_ID)], None);
    assert_eq!(prepare(&fixture, request), Err(FrameError::ScopeDenied));
}

#[test]
fn m2_06_revoked_scope_is_denied() {
    let fixture = Fixture::new(&[("task", CODING_ID)]);
    fixture.add_coding_job(1, "running", "running", "accepted");
    fixture
        .writer
        .write(|c| {
            c.execute(
                "UPDATE context_scopes SET state='revoked' WHERE scope_key='task:j1'",
                [],
            )
            .map_err(crate::database_error)?;
            c.execute(
                "UPDATE context_scope_epochs SET epoch=epoch+1 WHERE scope_key='task:j1'",
                [],
            )
            .map_err(crate::database_error)?;
            Ok(())
        })
        .unwrap();
    let access = fixture.access();
    let request = fixture.request(access, vec![fixture.coding_ref(CODING_ID)], None);
    assert_eq!(prepare(&fixture, request), Err(FrameError::ScopeDenied));
}

#[test]
fn m2_21_one_disallowed_reference_denies_the_whole_request() {
    let fixture = Fixture::new(&[("task", CODING_ID)]);
    fixture.add_coding_job(1, "running", "running", "accepted");
    let refs = vec![
        fixture.coding_ref(CODING_ID),
        fixture.coding_ref("not_registered"),
    ];
    let access = fixture.access();
    let request = fixture.request(access, refs, None);
    assert_eq!(prepare(&fixture, request), Err(FrameError::ScopeDenied));
}

#[test]
fn m2_21_same_name_in_another_project_is_denied() {
    let fixture = Fixture::new(&[("task", CODING_ID)]);
    fixture.add_coding_job(1, "running", "running", "accepted");
    // A second project with a same-named resource scope must not authorize.
    fixture
        .writer
        .write(|c| {
            c.execute(
                "INSERT INTO context_scopes(scope_key,kind,opaque_id,state,created_at)
                 VALUES('resource:shadow','resource','shadow','active','1')",
                [],
            )
            .map_err(crate::database_error)?;
            c.execute(
                "INSERT INTO context_scope_epochs(scope_key,epoch) VALUES('resource:shadow',0)",
                [],
            )
            .map_err(crate::database_error)?;
            Ok(())
        })
        .unwrap();
    let access = fixture.access();
    let request = fixture.request(access, vec![fixture.coding_ref("shadow")], None);
    assert_eq!(prepare(&fixture, request), Err(FrameError::ScopeDenied));
}

#[test]
fn m2_21_low_classification_never_leaks_runtime_content() {
    let fixture = Fixture::new(&[("task", CODING_ID)]);
    fixture.add_coding_job(1, "running", "running", "accepted");
    let mut access = fixture.access();
    access.max_classification = Classification::Public;
    let request = fixture.request(access, vec![fixture.coding_ref(CODING_ID)], None);
    let error = fixture.service().prepare_frame(request).unwrap_err();
    assert_eq!(error, FrameError::ScopeDenied);
    let text = error.code().to_string();
    assert!(!text.contains(CODING_ID));
}

#[test]
fn m2_21_invalid_reference_id_is_invalid_input() {
    let fixture = Fixture::new(&[("task", CODING_ID)]);
    let access = fixture.access();
    let bad = RuntimeRef {
        kind: saaa_personal_state_core::world::runtime_frame::RuntimeKind::CodingJob,
        id: "bad id".to_string(),
    };
    let request = fixture.request(access, vec![bad], None);
    assert_eq!(prepare(&fixture, request), Err(FrameError::InvalidInput));
}
