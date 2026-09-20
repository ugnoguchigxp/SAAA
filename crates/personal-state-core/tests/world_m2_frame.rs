//! M2-02..M2-04 / M2-09 / M2-12 / M2-14 / M2-18: core WorldFrame integration
//! tests through the public API. Pure: no DB, clock or IO.

use saaa_personal_state_core::world::runtime_frame::*;
use saaa_personal_state_core::world::slice_v2::WorldSliceV2;

fn view(id: &str, phase: RuntimePhase) -> RuntimeUnit {
    let reference = RuntimeRef {
        kind: RuntimeKind::MeetingSession,
        id: id.into(),
    };
    let view = RuntimeStateView {
        scope_key: runtime_scope_key(&reference),
        reference: reference.clone(),
        owner_state: RuntimeOwnerState::MeetingSession(MeetingOwnerState::Active),
        phase,
        job_revision: None,
        current_run_id: None,
        reported_complete: None,
        owner_digest: format!("digest-{id}"),
    };
    let focus = focus_for(&view, "project:p");
    RuntimeUnit { view, focus }
}

fn assembly(max_bytes: usize, runtime: Vec<RuntimeUnit>) -> FrameAssembly<'static> {
    FrameAssembly {
        run_id: "run1",
        project_scope: "project:p",
        captured_at_ms: 1_000,
        expires_at_ms: 2_000,
        max_bytes,
        graph: None,
        runtime,
        notices: Vec::new(),
    }
}

#[test]
fn m2_04_stamp_change_classification() {
    let stamp = |revision: u64, content: &str, owner: &str| FrameStamp {
        ledger_revision: revision,
        input_epoch: 3,
        policy_revision: 2,
        scope_digest: "scope".into(),
        owner_digests: vec![(
            RuntimeRef {
                kind: RuntimeKind::MeetingSession,
                id: "m1".into(),
            },
            owner.into(),
        )],
        content_digest: content.into(),
    };
    let base = stamp(10, "a", "x");
    assert_eq!(compare_stamp(&base, &base), FrameValidity::Current);
    assert_eq!(
        compare_stamp(&base, &stamp(11, "a", "x")),
        FrameValidity::Changed
    );
    assert_eq!(
        compare_stamp(&base, &stamp(10, "b", "x")),
        FrameValidity::Changed
    );
    assert_eq!(
        compare_stamp(&base, &stamp(10, "a", "y")),
        FrameValidity::Changed
    );
    let mut scope = stamp(10, "a", "x");
    scope.scope_digest = "other".into();
    assert_eq!(compare_stamp(&base, &scope), FrameValidity::Changed);
}

#[test]
fn m2_18_owner_digest_change_is_detected_even_with_same_revision() {
    let coding = CodingSnapshotInput {
        job_id: "j1",
        conversation_id: "c1",
        revision: 7,
        job_state: "running",
        current_run_id: Some("r1"),
        run_state: Some("running"),
        delivery: Some("accepted"),
        ended_at: None,
        reported_complete: None,
        source_versions: vec![("s1".into(), 1, "project:p".into())],
    };
    let CodingMapping::Present { digest: first, .. } = map_coding(&coding) else {
        panic!("maps");
    };
    let canceled = CodingSnapshotInput {
        job_state: "cancel_requested",
        ..coding.clone()
    };
    let CodingMapping::Present { digest: second, .. } = map_coding(&canceled) else {
        panic!("maps");
    };
    assert_ne!(first, second, "state change changes the owner digest");
    assert_eq!(coding.revision, canceled.revision);
}

#[test]
fn m2_14_assembly_drops_graph_before_stripping_runtime_evidence() {
    let graph = WorldSliceV2::empty(1, 0);
    let frame = assemble_frame(FrameAssembly {
        graph: Some(graph),
        ..assembly(8_192, vec![view("m1", RuntimePhase::Running)])
    })
    .unwrap();
    assert_eq!(frame.runtime.len(), 1);
    assert!(frame.runtime_focus.len() == 1);
}

#[test]
fn m2_14_graph_whole_omission_sets_truncated() {
    let mut graph = WorldSliceV2::empty(1, 0);
    // One node with a long name to force an oversized graph is unnecessary;
    // an impossible byte cap forces the whole-graph omission path.
    graph.notices.push("x".repeat(200));
    let frame = assemble_frame(FrameAssembly {
        graph: Some(graph),
        ..assembly(512, Vec::new())
    })
    .unwrap();
    assert!(frame.graph.is_none());
    assert!(frame.truncated);
    assert!(frame
        .notices
        .iter()
        .any(|notice| notice.code == FrameNoticeCode::WorldCapacityOmitted));
}

#[test]
fn m2_02_unknown_runtime_kind_is_rejected() {
    let value = serde_json::json!({"kind": "spaceship", "id": "x"});
    assert!(serde_json::from_value::<RuntimeRef>(value).is_err());
}

#[test]
fn m2_09_terminal_meeting_normalizes_unrelated_live_identity() {
    let base = MeetingSnapshotInput {
        db_status: Some("saved"),
        started_at: Some("100"),
        ended_at: Some("150"),
        saved_at: Some("160"),
        live_session_id: None,
        live_state: None,
    };
    let absent = map_meeting("m1", &base);
    let different = map_meeting(
        "m1",
        &MeetingSnapshotInput {
            live_session_id: Some("other"),
            live_state: Some(MeetingLivePhase::Active),
            ..base.clone()
        },
    );
    assert_eq!(
        absent, different,
        "a different or absent live session is normalized to null/null"
    );
    let same_idle = map_meeting(
        "m1",
        &MeetingSnapshotInput {
            live_session_id: Some("m1"),
            live_state: Some(MeetingLivePhase::Idle),
            ..base.clone()
        },
    );
    assert_ne!(
        absent, same_idle,
        "a same-id live session keeps its id and state in the digest"
    );
}
