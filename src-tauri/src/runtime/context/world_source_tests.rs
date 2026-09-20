use super::source::{Authority, Placement, Requirement};
use super::world_render::{parse_rendered_json, render_world_frame};
use super::world_source::{
    inspect_request, prepare_candidate, reset_prepare_calls, WorldOmission, WorldSourceOutcome,
    WorldSourceRequest, WORLD_SHADOW_KIND,
};
use crate::memory::personal_state::world::query::WorldSeed;
use crate::memory::personal_state::world::runtime_frame::GraphRequest;
use crate::memory::personal_state::world::runtime_test_support::{
    Fixture, CODING_ID, MEETING_ID, RUN_ID,
};
use crate::meeting::MeetingState;
use saaa_personal_state_core::world::runtime_frame::FrameValidity;
use saaa_personal_state_core::world::traversal_v2::{CausalDirection, LimitsV2};
use sha2::{Digest, Sha256};

fn inspect_ok(fixture: &Fixture, request: crate::memory::personal_state::world::runtime_frame::FrameRequest<'_>) {
    fixture
        .writer
        .read_serialized(|connection| {
            let scope = crate::runtime::context::scope::load(connection, RUN_ID).expect("scope");
            inspect_request(&WorldSourceRequest { frame_request: request }, &scope)
                .map_err(|omission| omission.as_str().to_string())
        })
        .expect("inspect");
}

#[test]
fn m3_01_request_types_are_not_serde_and_fields_stay_private() {
    assert!(!std::any::type_name::<WorldSourceRequest>().contains("Serialize"));
    let fixture = Fixture::new(&[("resource", MEETING_ID)]);
    fixture.add_meeting(MEETING_ID, "active", "100", None, None);
    fixture.meeting.set(Some(MEETING_ID), MeetingState::Active);
    let access = fixture.access();
    let request = fixture.request(access, vec![fixture.meeting_ref(MEETING_ID)], None);
    inspect_ok(&fixture, request);
}

#[test]
fn m3_02_exact_name_and_over_limit_seeds_are_rejected() {
    let fixture = Fixture::with_entities(&[], 1);
    let access = fixture.access();
    let mut graph = fixture.graph_request("ent0");
    graph.seeds = vec![WorldSeed::ExactName("同じ名前".into())];
    let request = fixture.request(access, Vec::new(), Some(graph));
    let omission = fixture
        .writer
        .read_serialized(|connection| {
            let scope = crate::runtime::context::scope::load(connection, RUN_ID).expect("scope");
            Ok(
                inspect_request(&WorldSourceRequest { frame_request: request }, &scope)
                    .expect_err("exact name"),
            )
        })
        .unwrap();
    assert_eq!(omission, WorldOmission::InvalidInput);

    let access = fixture.access();
    let graph = GraphRequest {
        seeds: (0..5)
            .map(|index| WorldSeed::EntityId(format!("ent{index}")))
            .collect(),
        causal_direction: CausalDirection::Forward,
        limits: LimitsV2::m1(),
        flags: crate::memory::personal_state::world::query_v2::IncludeFlags::default(),
        explicit_question: true,
    };
    let request = fixture.request(access, Vec::new(), Some(graph));
    let omission = fixture
        .writer
        .read_serialized(|connection| {
            let scope = crate::runtime::context::scope::load(connection, RUN_ID).expect("scope");
            Ok(
                inspect_request(&WorldSourceRequest { frame_request: request }, &scope)
                    .expect_err("five seeds"),
            )
        })
        .unwrap();
    assert_eq!(omission, WorldOmission::Limit);
}

#[test]
fn m3_02_four_seeds_and_eight_refs_are_accepted_empty_is_omitted() {
    let targets: Vec<(&str, &str)> = (1..=8)
        .map(|index| {
            let id: &'static str = Box::leak(format!("m{index}").into_boxed_str());
            ("resource", id)
        })
        .collect();
    let fixture = Fixture::with_entities(&targets, 4);
    let access = fixture.access();
    let graph = GraphRequest {
        seeds: (0..4)
            .map(|index| WorldSeed::EntityId(format!("ent{index}")))
            .collect(),
        causal_direction: CausalDirection::Forward,
        limits: LimitsV2::m1(),
        flags: crate::memory::personal_state::world::query_v2::IncludeFlags::default(),
        explicit_question: true,
    };
    let refs: Vec<_> = (1..=8)
        .map(|index| fixture.meeting_ref(&format!("m{index}")))
        .collect();
    let request = fixture.request(access, refs, Some(graph));
    inspect_ok(&fixture, request);

    let access = fixture.access();
    let request = fixture.request(access, Vec::new(), None);
    let omission = fixture
        .writer
        .read_serialized(|connection| {
            let scope = crate::runtime::context::scope::load(connection, RUN_ID).expect("scope");
            Ok(
                inspect_request(&WorldSourceRequest { frame_request: request }, &scope)
                    .expect_err("empty"),
            )
        })
        .unwrap();
    assert_eq!(omission, WorldOmission::EmptyRequest);
}

#[test]
fn m3_02_nine_refs_and_missing_project_are_omitted() {
    let fixture = Fixture::new(&[("resource", MEETING_ID)]);
    let access = fixture.access();
    let refs: Vec<_> = (1..=9)
        .map(|index| fixture.meeting_ref(&format!("x{index}")))
        .collect();
    let request = fixture.request(access, refs, None);
    let omission = fixture
        .writer
        .read_serialized(|connection| {
            let scope = crate::runtime::context::scope::load(connection, RUN_ID).expect("scope");
            Ok(
                inspect_request(&WorldSourceRequest { frame_request: request }, &scope)
                    .expect_err("nine refs"),
            )
        })
        .unwrap();
    assert_eq!(omission, WorldOmission::Limit);

    let access = fixture.access();
    let mut request = fixture.request(access, vec![fixture.meeting_ref(MEETING_ID)], None);
    request.project_scope = "";
    let omission = fixture
        .writer
        .read_serialized(|connection| {
            let scope = crate::runtime::context::scope::load(connection, RUN_ID).expect("scope");
            Ok(
                inspect_request(&WorldSourceRequest { frame_request: request }, &scope)
                    .expect_err("no project"),
            )
        })
        .unwrap();
    assert_eq!(omission, WorldOmission::NoExplicitProject);
}

#[test]
fn m3_05_one_frame_becomes_one_untrusted_candidate() {
    let fixture = Fixture::new(&[("resource", MEETING_ID)]);
    fixture.add_meeting(MEETING_ID, "active", "100", None, None);
    fixture.meeting.set(Some(MEETING_ID), MeetingState::Active);
    let service = fixture.service();
    let access = fixture.access();
    let request = fixture.request(access, vec![fixture.meeting_ref(MEETING_ID)], None);
    let ready = fixture
        .writer
        .read_serialized(|connection| {
            match prepare_candidate(
                &service,
                connection,
                WorldSourceRequest {
                    frame_request: request,
                },
            ) {
                WorldSourceOutcome::Ready(ready) => Ok(ready.candidate().clone()),
                WorldSourceOutcome::Omitted(omission) => {
                    Err(format!("omitted {}", omission.as_str()))
                }
            }
        })
        .expect("candidate");
    assert_eq!(ready.source_kind, WORLD_SHADOW_KIND);
    assert_eq!(ready.requirement, Requirement::May);
    assert_eq!(ready.authority, Authority::UntrustedData);
    assert_eq!(ready.placement, Placement::Base);
    assert_eq!(ready.utility, 0);
    assert_eq!(ready.source_version, 1);
    assert_eq!(ready.cost_bytes, ready.content.len());
    let digest = format!("{:x}", Sha256::digest(ready.content.as_bytes()));
    assert_eq!(ready.source_digest, digest);
    assert_eq!(ready.source_id, format!("world-frame:{RUN_ID}:{digest}"));
    assert_eq!(ready.candidate_id, ready.source_id);
    assert!(ready.scope_refs.contains(&fixture.project));
}

#[test]
fn m3_06_prepare_and_revalidate_use_one_service() {
    let fixture = Fixture::file(&[("resource", MEETING_ID)], 0, false);
    fixture.add_meeting(MEETING_ID, "active", "100", None, None);
    fixture.meeting.set(Some(MEETING_ID), MeetingState::Active);
    let service = fixture.service();
    let access = fixture.access();
    let request = fixture.request(access, vec![fixture.meeting_ref(MEETING_ID)], None);
    let ready = fixture
        .writer
        .read_serialized(|connection| {
            match prepare_candidate(
                &service,
                connection,
                WorldSourceRequest {
                    frame_request: request,
                },
            ) {
                WorldSourceOutcome::Ready(ready) => Ok(ready),
                WorldSourceOutcome::Omitted(omission) => {
                    Err(format!("omitted {}", omission.as_str()))
                }
            }
        })
        .expect("ready");
    assert!(matches!(
        service.revalidate_frame(ready.prepared()).unwrap(),
        FrameValidity::Current
    ));
    let other = fixture.service();
    assert!(matches!(
        other.revalidate_frame(ready.prepared()).unwrap(),
        FrameValidity::Expired
    ));
}

#[test]
fn m3_07_ttl_boundaries_and_scope_denial_on_file_db() {
    let fixture = Fixture::file(&[("resource", MEETING_ID)], 1, false);
    fixture.add_meeting(MEETING_ID, "active", "100", None, None);
    fixture.meeting.set(Some(MEETING_ID), MeetingState::Active);
    let service = fixture.service();
    let access = fixture.access();
    let request = fixture.request(
        access,
        vec![fixture.meeting_ref(MEETING_ID)],
        Some(fixture.graph_request("ent0")),
    );
    let ready = fixture
        .writer
        .read_serialized(|connection| {
            match prepare_candidate(
                &service,
                connection,
                WorldSourceRequest {
                    frame_request: request,
                },
            ) {
                WorldSourceOutcome::Ready(ready) => Ok(ready),
                WorldSourceOutcome::Omitted(omission) => {
                    Err(format!("omitted {}", omission.as_str()))
                }
            }
        })
        .expect("prepared at 1000");
    fixture.set_now(1_999);
    assert!(matches!(
        service.revalidate_frame(ready.prepared()).unwrap(),
        FrameValidity::Current
    ));
    fixture.set_now(2_000);
    assert!(matches!(
        service.revalidate_frame(ready.prepared()).unwrap(),
        FrameValidity::Expired
    ));
    fixture.set_now(999);
    assert!(matches!(
        service.revalidate_frame(ready.prepared()).unwrap(),
        FrameValidity::Expired
    ));

    fixture.set_now(1_000);
    fixture
        .writer
        .write(|connection| {
            connection
                .execute(
                    "DELETE FROM context_scope_links WHERE parent_scope_key=?1 AND child_scope_key=?2",
                    [fixture.project.clone(), format!("resource:{MEETING_ID}")],
                )
                .map_err(crate::database_error)?;
            Ok(())
        })
        .unwrap();
    let access = fixture.access();
    let request = fixture.request(access, vec![fixture.meeting_ref(MEETING_ID)], None);
    let omission = fixture
        .writer
        .read_serialized(|connection| {
            Ok(match prepare_candidate(
                &service,
                connection,
                WorldSourceRequest {
                    frame_request: request,
                },
            ) {
                WorldSourceOutcome::Omitted(omission) => omission,
                WorldSourceOutcome::Ready(_) => {
                    return Err("expected denial".into());
                }
            })
        })
        .unwrap();
    assert_eq!(omission, WorldOmission::ScopeDenied);
}

#[test]
fn m3_07_notice_only_frame_is_empty() {
    let fixture = Fixture::new(&[]);
    let mut graph = fixture.graph_request("missing");
    graph.seeds.clear();
    let access = fixture.access();
    let request = fixture.request(access, Vec::new(), Some(graph));
    reset_prepare_calls();
    let omission = fixture
        .writer
        .read_serialized(|connection| {
            Ok(match prepare_candidate(
                &fixture.service(),
                connection,
                WorldSourceRequest {
                    frame_request: request,
                },
            ) {
                WorldSourceOutcome::Omitted(omission) => omission,
                WorldSourceOutcome::Ready(_) => return Err("expected empty".into()),
            })
        })
        .unwrap();
    assert_eq!(omission, WorldOmission::EmptyRequest);
    let _ = (render_world_frame, parse_rendered_json, CODING_ID);
}
