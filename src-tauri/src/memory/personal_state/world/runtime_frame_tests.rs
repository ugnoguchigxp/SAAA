#![cfg(test)]

//! M2-16 / M2-17 / M2-19 / M2-24 / M2-26: the WorldFrame service lifecycle,
//! revalidation, graph omission and budget boundaries.

use super::runtime_test_support::*;
use saaa_personal_state_core::world::runtime_frame::{
    FrameNoticeCode, FrameValidity, RuntimePhase,
};

#[test]
fn m2_16_instance_id_is_stable_per_service_and_unique_across_services() {
    let fixture = Fixture::new(&[("task", CODING_ID)]);
    fixture.add_coding_job(1, "running", "running", "accepted");
    let service = fixture.service();
    let access = fixture.access();
    let first = service
        .prepare_frame(fixture.request(access, vec![fixture.coding_ref(CODING_ID)], None))
        .unwrap();
    let access = fixture.access();
    let second = service
        .prepare_frame(fixture.request(access, vec![fixture.coding_ref(CODING_ID)], None))
        .unwrap();
    assert_eq!(first.instance_id(), second.instance_id());
    let other = fixture.service();
    let access = fixture.access();
    let rebuilt = other
        .prepare_frame(fixture.request(access, vec![fixture.coding_ref(CODING_ID)], None))
        .unwrap();
    assert_ne!(first.instance_id(), rebuilt.instance_id());
    // A frame prepared by another service instance is expired, never current.
    assert_eq!(
        other.revalidate_frame(&first).unwrap(),
        FrameValidity::Expired
    );
}

#[test]
fn m2_17_read_path_writes_nothing() {
    let fixture = Fixture::new(&[("task", CODING_ID)]);
    fixture.add_coding_job(1, "running", "running", "accepted");
    let before = fixture.total_changes();
    let service = fixture.service();
    let access = fixture.access();
    let request = fixture.request(access, vec![fixture.coding_ref(CODING_ID)], None);
    let prepared = service.prepare_frame(request).unwrap();
    let _ = service.revalidate_frame(&prepared).unwrap();
    assert_eq!(fixture.total_changes(), before);
}

#[test]
fn m2_19_scope_link_removal_makes_the_old_frame_scope_denied() {
    let fixture = Fixture::new(&[("task", CODING_ID)]);
    fixture.add_coding_job(1, "running", "running", "accepted");
    let service = fixture.service();
    let access = fixture.access();
    let request = fixture.request(access, vec![fixture.coding_ref(CODING_ID)], None);
    let prepared = service.prepare_frame(request).unwrap();
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
    assert_eq!(
        service.revalidate_frame(&prepared).unwrap(),
        FrameValidity::ScopeDenied
    );
    // The stored frame is not rewritten into Current.
    assert_eq!(prepared.frame().runtime.len(), 1);
}

#[test]
fn m2_19_source_content_change_makes_the_old_frame_changed() {
    let fixture = Fixture::new(&[("task", CODING_ID)]);
    fixture.add_coding_job(1, "running", "running", "accepted");
    let service = fixture.service();
    let access = fixture.access();
    let request = fixture.request(access, vec![fixture.coding_ref(CODING_ID)], None);
    let prepared = service.prepare_frame(request).unwrap();
    fixture
        .writer
        .write(|c| {
            c.execute(
                "UPDATE conversation_messages SET content='edited' WHERE id=?1",
                [MESSAGE_ID],
            )
            .map_err(crate::database_error)?;
            Ok(())
        })
        .unwrap();
    let validity = service.revalidate_frame(&prepared).unwrap();
    assert!(
        matches!(
            validity,
            FrameValidity::Changed | FrameValidity::ScopeDenied
        ),
        "source edit invalidates the frame: {validity:?}"
    );
}

#[test]
fn m2_19_clock_rollback_is_expired() {
    let fixture = Fixture::new(&[("task", CODING_ID)]);
    fixture.add_coding_job(1, "running", "running", "accepted");
    let service = fixture.service();
    let access = fixture.access();
    let request = fixture.request(access, vec![fixture.coding_ref(CODING_ID)], None);
    let prepared = service.prepare_frame(request).unwrap();
    fixture.set_now(999);
    assert_eq!(
        service.revalidate_frame(&prepared).unwrap(),
        FrameValidity::Expired
    );
    fixture.set_now(1_999);
    assert_eq!(
        service.revalidate_frame(&prepared).unwrap(),
        FrameValidity::Current
    );
    fixture.set_now(2_000);
    assert_eq!(
        service.revalidate_frame(&prepared).unwrap(),
        FrameValidity::Expired
    );
}

#[test]
fn m2_24_graph_and_runtime_are_returned_together() {
    let fixture = Fixture::with_entities(&[("task", CODING_ID)], 2);
    fixture.add_coding_job(1, "running", "running", "accepted");
    let access = fixture.access();
    let request = fixture.request(
        access,
        vec![fixture.coding_ref(CODING_ID)],
        Some(fixture.graph_request("ent0")),
    );
    let frame = fixture
        .service()
        .prepare_frame(request)
        .unwrap()
        .frame()
        .clone();
    assert!(frame.graph.is_some());
    assert_eq!(frame.runtime.len(), 1);
    assert!(frame.runtime[0].phase == RuntimePhase::Running);
}

#[test]
fn m2_24_pending_projection_omits_graph_but_keeps_runtime() {
    let fixture = Fixture::build(&[("task", CODING_ID)], 2, true);
    fixture.add_coding_job(1, "running", "running", "accepted");
    let access = fixture.access();
    let request = fixture.request(
        access,
        vec![fixture.coding_ref(CODING_ID)],
        Some(fixture.graph_request("ent0")),
    );
    let frame = fixture
        .service()
        .prepare_frame(request)
        .unwrap()
        .frame()
        .clone();
    assert!(frame.graph.is_none());
    assert_eq!(frame.runtime.len(), 1);
    assert!(frame
        .notices
        .iter()
        .any(|n| n.code == FrameNoticeCode::WorldPending));
}

#[test]
fn world_review_projection_above_the_old_limit_keeps_graph_and_runtime() {
    let fixture = Fixture::with_entities(&[("task", CODING_ID)], 101);
    fixture.add_coding_job(1, "running", "running", "accepted");
    let access = fixture.access();
    let request = fixture.request(
        access,
        vec![fixture.coding_ref(CODING_ID)],
        Some(fixture.graph_request("ent0")),
    );
    let frame = fixture
        .service()
        .prepare_frame(request)
        .unwrap()
        .frame()
        .clone();
    assert!(frame.graph.is_some());
    assert_eq!(frame.runtime.len(), 1);
    assert!(!frame
        .notices
        .iter()
        .any(|n| n.code == FrameNoticeCode::WorldCapacityOmitted));
}

#[test]
fn m2_26_budget_is_never_exceeded_and_notices_are_bounded() {
    let fixture = Fixture::with_entities(&[("task", CODING_ID)], 4);
    fixture.add_coding_job(1, "running", "running", "accepted");
    let service = fixture.service();
    for max_bytes in [600usize, 800, 1_200, 2_000, 8_192] {
        let access = fixture.access();
        let mut request = fixture.request(
            access,
            vec![fixture.coding_ref(CODING_ID)],
            Some(fixture.graph_request("ent0")),
        );
        request.max_bytes = max_bytes;
        let frame = service.prepare_frame(request).unwrap().frame().clone();
        assert!(frame.encoded_len().unwrap() <= max_bytes);
        assert!(frame.notices.len() <= 16);
        assert!(frame.runtime.len() + frame.graph.as_ref().map_or(0, |g| g.nodes.len()) <= 30);
    }
}

#[test]
fn m2_26_impossible_budget_is_rejected() {
    let fixture = Fixture::new(&[("task", CODING_ID)]);
    let access = fixture.access();
    let mut request = fixture.request(access, vec![], None);
    request.max_bytes = 1;
    assert_eq!(
        fixture.service().prepare_frame(request).unwrap_err().code(),
        "frame-budget-too-small"
    );
}

#[test]
fn m2_26_long_japanese_content_preserves_evidence() {
    let fixture = Fixture::with_entities(&[("task", CODING_ID)], 4);
    fixture.add_coding_job(1, "running", "running", "accepted");
    let access = fixture.access();
    let request = fixture.request(
        access,
        vec![fixture.coding_ref(CODING_ID)],
        Some(fixture.graph_request("ent0")),
    );
    let frame = fixture
        .service()
        .prepare_frame(request)
        .unwrap()
        .frame()
        .clone();
    if let Some(graph) = &frame.graph {
        // Whole relations are adopted or omitted; conditions/evidence are not
        // partially stripped.
        for relation in &graph.relations {
            assert!(!relation.semantic_key.is_empty());
        }
    }
    assert!(frame.encoded_len().unwrap() <= 8_192);
}

#[test]
fn m2_19_ledger_revision_change_invalidates_the_old_frame() {
    use super::test_support::{v2_entity_assertion, Committer, PROJECT};
    use saaa_personal_state_core::world::model_v2::EntityKindV2;
    let fixture = Fixture::with_entities(&[("task", CODING_ID)], 2);
    fixture.add_coding_job(1, "running", "running", "accepted");
    let service = fixture.service();
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

    // A new World assertion changes the ledger revision, so even before the TTL
    // the old frame must be rejected.
    let now_ms = crate::memory::personal_state::now();
    let source = fixture
        .writer
        .write(|c| {
            Ok(super::test_support::insert_source(
                c,
                PROJECT,
                "s_new",
                "new source",
            ))
        })
        .unwrap();
    let mut committer = Committer {
        writer: fixture.writer.as_ref(),
        project: PROJECT,
    };
    committer
        .commit(
            "m2-new-entity",
            vec![v2_entity_assertion(
                "e_new",
                "ent_new",
                EntityKindV2::Concept,
                "新規",
                &[],
                None,
                &source,
                PROJECT,
                now_ms,
            )],
        )
        .expect("new assertion commits");
    assert_eq!(
        service.revalidate_frame(&prepared).unwrap(),
        FrameValidity::Changed
    );
}
#[test]
fn m2_19_source_forget_invalidates_the_old_frame() {
    let fixture = Fixture::with_entities(&[("task", CODING_ID)], 2);
    fixture.add_coding_job(1, "running", "running", "accepted");
    let service = fixture.service();
    let access = fixture.access();
    let request = fixture.request(
        access,
        vec![fixture.coding_ref(CODING_ID)],
        Some(fixture.graph_request("ent0")),
    );
    let prepared = service.prepare_frame(request).unwrap();
    // Forget the world source; the projection is wiped and the epoch changes.
    fixture
        .writer
        .write(|c| {
            c.execute("DELETE FROM conversation_messages WHERE id='s1'", [])
                .map_err(crate::database_error)?;
            Ok(())
        })
        .unwrap();
    let validity = service.revalidate_frame(&prepared).unwrap();
    assert!(
        matches!(
            validity,
            FrameValidity::Changed | FrameValidity::ScopeDenied
        ),
        "source forget invalidates: {validity:?}"
    );
}

#[test]
fn world_review_projection_100_and_101_are_both_allowed() {
    let allowed = Fixture::with_entities(&[("task", CODING_ID)], 100);
    allowed.add_coding_job(1, "running", "running", "accepted");
    let access = allowed.access();
    let request = allowed.request(
        access,
        vec![allowed.coding_ref(CODING_ID)],
        Some(allowed.graph_request("ent0")),
    );
    let frame = allowed
        .service()
        .prepare_frame(request)
        .unwrap()
        .frame()
        .clone();
    assert!(frame.graph.is_some(), "100 projected entities are allowed");

    let omitted = Fixture::with_entities(&[("task", CODING_ID)], 101);
    omitted.add_coding_job(1, "running", "running", "accepted");
    let access = omitted.access();
    let request = omitted.request(
        access,
        vec![omitted.coding_ref(CODING_ID)],
        Some(omitted.graph_request("ent0")),
    );
    let frame = omitted
        .service()
        .prepare_frame(request)
        .unwrap()
        .frame()
        .clone();
    assert!(
        frame.graph.is_some(),
        "101 projected entities remain queryable"
    );
}

#[test]
fn world_review_unrelated_ledger_rows_do_not_disable_graph() {
    let fixture = Fixture::with_entities(&[("task", CODING_ID)], 2);
    fixture.add_coding_job(1, "running", "running", "accepted");
    let current = fixture.ledger_count() as usize;
    fixture.fill_coverage(2_000 - current);
    assert_eq!(fixture.ledger_count(), 2_000);
    let access = fixture.access();
    let request = fixture.request(
        access,
        vec![fixture.coding_ref(CODING_ID)],
        Some(fixture.graph_request("ent0")),
    );
    let frame = fixture
        .service()
        .prepare_frame(request)
        .unwrap()
        .frame()
        .clone();
    assert!(frame.graph.is_some(), "ledger at the limit is allowed");

    fixture.fill_coverage(1);
    assert_eq!(fixture.ledger_count(), 2_001);
    let access = fixture.access();
    let request = fixture.request(
        access,
        vec![fixture.coding_ref(CODING_ID)],
        Some(fixture.graph_request("ent0")),
    );
    let frame = fixture
        .service()
        .prepare_frame(request)
        .unwrap()
        .frame()
        .clone();
    assert!(
        frame.graph.is_some(),
        "unrelated global ledger growth must not suppress this project"
    );
}

#[test]
fn m2_15_empty_seed_does_not_fabricate_a_node() {
    let fixture = Fixture::with_entities(&[("task", CODING_ID)], 2);
    fixture.add_coding_job(1, "running", "running", "accepted");
    let mut graph = fixture.graph_request("ent0");
    graph.seeds.clear();
    graph.explicit_question = false;
    let access = fixture.access();
    let request = fixture.request(access, vec![fixture.coding_ref(CODING_ID)], Some(graph));
    let frame = fixture
        .service()
        .prepare_frame(request)
        .unwrap()
        .frame()
        .clone();
    let graph = frame.graph.expect("graph envelope is still returned");
    assert!(
        graph.nodes.is_empty(),
        "no node is fabricated from an empty seed"
    );
    assert_eq!(frame.runtime.len(), 1);
}

#[test]
fn m2_15_five_distinct_seeds_are_a_limit() {
    use super::query::WorldSeed;
    let fixture = Fixture::with_entities(&[("task", CODING_ID)], 2);
    fixture.add_coding_job(1, "running", "running", "accepted");
    let mut graph = fixture.graph_request("ent0");
    graph.seeds = (0..5)
        .map(|index| WorldSeed::EntityId(format!("ent{index}")))
        .collect();
    let access = fixture.access();
    let request = fixture.request(access, vec![fixture.coding_ref(CODING_ID)], Some(graph));
    assert_eq!(
        fixture.service().prepare_frame(request).unwrap_err().code(),
        "frame-limit"
    );
}

#[test]
fn m2_15_duplicate_seeds_are_deduplicated_not_truncated() {
    use super::query::WorldSeed;
    let fixture = Fixture::with_entities(&[("task", CODING_ID)], 2);
    fixture.add_coding_job(1, "running", "running", "accepted");
    let mut graph = fixture.graph_request("ent0");
    graph.seeds = vec![
        WorldSeed::EntityId("ent0".into()),
        WorldSeed::EntityId("ent0".into()),
        WorldSeed::ExactName("名前0".into()),
        WorldSeed::ExactName("名前0".into()),
    ];
    let access = fixture.access();
    let request = fixture.request(access, vec![fixture.coding_ref(CODING_ID)], Some(graph));
    let frame = fixture
        .service()
        .prepare_frame(request)
        .unwrap()
        .frame()
        .clone();
    assert!(frame.graph.is_some());
}

#[test]
fn m2_19_deleted_run_input_source_denies_scope() {
    let fixture = Fixture::new(&[("task", CODING_ID)]);
    fixture.add_coding_job(1, "running", "running", "accepted");
    let service = fixture.service();
    let access = fixture.access();
    let request = fixture.request(access, vec![fixture.coding_ref(CODING_ID)], None);
    let prepared = service.prepare_frame(request).unwrap();
    fixture
        .writer
        .write(|c| {
            c.execute(
                "DELETE FROM conversation_messages WHERE id=?1",
                [MESSAGE_ID],
            )
            .map_err(crate::database_error)?;
            Ok(())
        })
        .unwrap();
    assert_eq!(
        service.revalidate_frame(&prepared).unwrap(),
        FrameValidity::ScopeDenied
    );
}

#[test]
fn m2_26_small_budget_with_graph_omits_graph_not_frame() {
    let fixture = Fixture::with_entities(&[("task", CODING_ID)], 4);
    fixture.add_coding_job(1, "running", "running", "accepted");
    let access = fixture.access();
    let mut request = fixture.request(
        access,
        vec![fixture.coding_ref(CODING_ID)],
        Some(fixture.graph_request("ent0")),
    );
    // v2 carries explicit scope authorization; 320 bytes no longer fits its header.
    request.max_bytes = 320;
    assert!(fixture.service().prepare_frame(request).is_err());
    let mut request = fixture.request(
        fixture.access(),
        vec![fixture.coding_ref(CODING_ID)],
        Some(fixture.graph_request("ent0")),
    );
    let header = fixture
        .service()
        .prepare_frame(fixture.request(fixture.access(), vec![], None))
        .unwrap();
    let budget = header.frame().encoded_len().unwrap() + 128;
    request.max_bytes = budget;
    let frame = fixture
        .service()
        .prepare_frame(request)
        .unwrap()
        .frame()
        .clone();
    assert!(frame.graph.is_none());
    assert!(frame.encoded_len().unwrap() <= budget);
}

#[test]
fn world_g1_revalidation_cannot_renew_a_deadline_during_rebuild() {
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };
    let fixture = Fixture::new(&[]);
    let calls = Arc::new(AtomicUsize::new(0));
    let counter = calls.clone();
    let service = super::runtime_frame::WorldFrameService::new(
        fixture.readers(),
        Arc::new(move || {
            // prepare: start/end; revalidate: start/rebuild/end. The deadline crosses at end.
            if counter.fetch_add(1, Ordering::SeqCst) >= 4 {
                2_000
            } else {
                1_000
            }
        }),
    );
    let prepared = service
        .prepare_frame(fixture.request(fixture.access(), Vec::new(), None))
        .unwrap();
    assert_eq!(
        service.revalidate_frame(&prepared).unwrap(),
        FrameValidity::Expired
    );
}
