#![cfg(test)]

//! M2-25: persistent-reader snapshot integrity. Uses a real temporary file DB so
//! the production read transaction path is exercised, not the serialized test
//! backend. Reader connections are read-only.

use super::runtime_test_support::*;
use saaa_personal_state_core::world::runtime_frame::FrameValidity;

#[test]
fn m2_25_persistent_readers_report_current_then_changed() {
    let fixture = Fixture::file(&[("task", CODING_ID)], 2, false);
    fixture.add_coding_job(1, "running", "running", "accepted");
    let clock = fixture.clock.clone();
    let service = super::runtime_frame::WorldFrameService::new(fixture.readers_open(), std::sync::Arc::new(move || clock.load(std::sync::atomic::Ordering::SeqCst)),
    );
    let access = fixture.access();
    let request = fixture.request(
        access,
        vec![fixture.coding_ref(CODING_ID)],
        Some(fixture.graph_request("ent0")),
    );
    let prepared = service.prepare_frame(request).unwrap();
    assert_eq!(
        service.revalidate_frame(&prepared).unwrap(),
        FrameValidity::Current
    );

    // A coding owner change between the two reads rejects the old frame.
    fixture.set_coding_state(1, "cancel_requested", "running", "accepted");
    assert_eq!(
        service.revalidate_frame(&prepared).unwrap(),
        FrameValidity::Changed
    );
}

#[test]
fn m2_25_policy_and_epoch_changes_are_rejected() {
    let fixture = Fixture::file(&[("task", CODING_ID)], 2, false);
    fixture.add_coding_job(1, "running", "running", "accepted");
    let clock = fixture.clock.clone();
    let service = super::runtime_frame::WorldFrameService::new(fixture.readers_open(), std::sync::Arc::new(move || clock.load(std::sync::atomic::Ordering::SeqCst)),
    );
    let access = fixture.access();
    let request = fixture.request(
        access,
        vec![fixture.coding_ref(CODING_ID)],
        Some(fixture.graph_request("ent0")),
    );
    let prepared = service.prepare_frame(request).unwrap();

    fixture
        .writer
        .write(|c| {
            c.execute(
                "UPDATE personal_scope SET policy_revision=policy_revision+1 WHERE id='primary'",
                [],
            )
            .map_err(crate::database_error)?;
            Ok(())
        })
        .unwrap();
    let policy = service.revalidate_frame(&prepared).unwrap();
    assert!(
        matches!(policy, FrameValidity::Changed | FrameValidity::ScopeDenied),
        "policy change rejects: {policy:?}"
    );
}

#[test]
fn m2_25_scope_epoch_change_is_rejected() {
    let fixture = Fixture::file(&[("task", CODING_ID)], 2, false);
    fixture.add_coding_job(1, "running", "running", "accepted");
    let clock = fixture.clock.clone();
    let service = super::runtime_frame::WorldFrameService::new(fixture.readers_open(), std::sync::Arc::new(move || clock.load(std::sync::atomic::Ordering::SeqCst)),
    );
    let access = fixture.access();
    let request = fixture.request(access, vec![fixture.coding_ref(CODING_ID)], None);
    let prepared = service.prepare_frame(request).unwrap();

    fixture
        .writer
        .write(|c| {
            c.execute(
                "UPDATE context_scope_epochs SET epoch=epoch+1 WHERE scope_key='task:j1'",
                [],
            )
            .map_err(crate::database_error)?;
            Ok(())
        })
        .unwrap();
    let epoch = service.revalidate_frame(&prepared).unwrap();
    assert!(
        matches!(epoch, FrameValidity::Changed | FrameValidity::ScopeDenied),
        "epoch change rejects: {epoch:?}"
    );
}

#[test]
fn m2_25_reader_connections_reject_writes() {
    let fixture = Fixture::file(&[("task", CODING_ID)], 2, false);
    let readers = fixture.readers_open();
    let result = readers.read(|c| {
        c.execute(
            "INSERT INTO personal_tombstones(source_id,forgotten_at) VALUES('x',1)",
            [],
        )
        .map(|_| ())
        .map_err(crate::database_error)
    });
    assert!(result.is_err(), "reader must be read-only");
}
