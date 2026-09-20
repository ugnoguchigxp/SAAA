use super::super::source::{Authority, Placement, Requirement};
use super::source::{
    inspect_request, prepare_candidate, reset_prepare_calls, PreparedWorldCandidate, WorldOmission,
    WorldSourceOutcome, WorldSourceRequest, WORLD_SHADOW_KIND,
};
use crate::meeting::MeetingState;
use crate::memory::personal_state::world::query::WorldSeed;
use crate::memory::personal_state::world::runtime_frame::{
    FrameRequest, GraphRequest, WorldFrameService,
};
use crate::memory::personal_state::world::runtime_test_support::{Fixture, MEETING_ID, RUN_ID};
use crate::runtime::context::scope::ScopeSnapshot;
use saaa_personal_state_core::world::runtime_frame::FrameValidity;
use saaa_personal_state_core::world::traversal_v2::{CausalDirection, LimitsV2};
use sha2::{Digest, Sha256};

fn load_scope(fixture: &Fixture) -> ScopeSnapshot {
    fixture
        .writer
        .read_serialized(|connection| crate::runtime::context::scope::load(connection, RUN_ID))
        .expect("scope")
}

fn inspect(fixture: &Fixture, request: FrameRequest<'_>) -> Result<(), WorldOmission> {
    inspect_request(
        &WorldSourceRequest {
            frame_request: request,
        },
        &load_scope(fixture),
    )
}

fn prepare(
    service: &WorldFrameService,
    fixture: &Fixture,
    request: FrameRequest<'_>,
) -> Result<Box<PreparedWorldCandidate>, WorldOmission> {
    match prepare_candidate(
        service,
        WorldSourceRequest {
            frame_request: request,
        },
        &load_scope(fixture),
    ) {
        WorldSourceOutcome::Ready(ready) => Ok(ready),
        WorldSourceOutcome::Omitted(omission) => Err(omission),
    }
}

fn entity_graph(count: usize) -> GraphRequest {
    GraphRequest {
        seeds: (0..count)
            .map(|index| WorldSeed::EntityId(format!("ent{index}")))
            .collect(),
        causal_direction: CausalDirection::Forward,
        limits: LimitsV2::m1(),
        flags: crate::memory::personal_state::world::query_v2::IncludeFlags::default(),
        explicit_question: true,
    }
}

fn meeting_fixture() -> Fixture {
    let fixture = Fixture::new(&[("resource", MEETING_ID)]);
    fixture.add_meeting(MEETING_ID, "active", "100", None, None);
    fixture.meeting.set(Some(MEETING_ID), MeetingState::Active);
    fixture
}

#[test]
fn m3_01_request_types_are_not_serde_and_fields_stay_private() {
    assert!(!std::any::type_name::<WorldSourceRequest>().contains("Serialize"));
    let fixture = meeting_fixture();
    let access = fixture.access();
    inspect(
        &fixture,
        fixture.request(access, vec![fixture.meeting_ref(MEETING_ID)], None),
    )
    .expect("inspect");
}

#[test]
fn m3_02_exact_name_and_over_limit_seeds_are_rejected() {
    let fixture = Fixture::with_entities(&[], 1);
    let access = fixture.access();
    let mut graph = fixture.graph_request("ent0");
    graph.seeds = vec![WorldSeed::ExactName("同じ名前".into())];
    let request = fixture.request(access, Vec::new(), Some(graph));
    assert_eq!(
        inspect(&fixture, request).expect_err("exact name"),
        WorldOmission::InvalidInput
    );

    let access = fixture.access();
    let request = fixture.request(access, Vec::new(), Some(entity_graph(5)));
    assert_eq!(
        inspect(&fixture, request).expect_err("five seeds"),
        WorldOmission::Limit
    );
}

#[test]
fn m3_02_four_seeds_and_eight_refs_are_accepted_empty_is_omitted() {
    let meeting_ids: Vec<String> = (1..=8).map(|index| format!("m{index}")).collect();
    let targets: Vec<(&str, &str)> = meeting_ids
        .iter()
        .map(|id| ("resource", id.as_str()))
        .collect();
    let fixture = Fixture::with_entities(&targets, 4);
    let access = fixture.access();
    let refs: Vec<_> = (1..=8)
        .map(|index| fixture.meeting_ref(&format!("m{index}")))
        .collect();
    inspect(
        &fixture,
        fixture.request(access, refs, Some(entity_graph(4))),
    )
    .expect("four seeds");

    let access = fixture.access();
    let mut graph = entity_graph(4);
    graph.seeds.push(WorldSeed::EntityId("ent0".into()));
    inspect(&fixture, fixture.request(access, Vec::new(), Some(graph))).expect("deduped seeds");

    let access = fixture.access();
    assert_eq!(
        inspect(&fixture, fixture.request(access, Vec::new(), None)).expect_err("empty"),
        WorldOmission::EmptyRequest
    );
}

#[test]
fn m3_02_nine_refs_and_missing_project_are_omitted() {
    let fixture = Fixture::new(&[("resource", MEETING_ID)]);
    let access = fixture.access();
    let refs: Vec<_> = (1..=9)
        .map(|index| fixture.meeting_ref(&format!("x{index}")))
        .collect();
    assert_eq!(
        inspect(&fixture, fixture.request(access, refs, None)).expect_err("nine refs"),
        WorldOmission::Limit
    );

    let access = fixture.access();
    let mut request = fixture.request(access, vec![fixture.meeting_ref(MEETING_ID)], None);
    request.project_scope = "";
    assert_eq!(
        inspect(&fixture, request).expect_err("no project"),
        WorldOmission::NoExplicitProject
    );

    let access = fixture.access();
    let mut request = fixture.request(access, vec![fixture.meeting_ref(MEETING_ID)], None);
    request.project_scope = "project:p,project:q";
    assert_eq!(
        inspect(&fixture, request).expect_err("ambiguous"),
        WorldOmission::AmbiguousProject
    );

    let access = fixture.access();
    let mut request = fixture.request(access, vec![fixture.meeting_ref(MEETING_ID)], None);
    request.project_scope = "project:other";
    assert_eq!(
        inspect(&fixture, request).expect_err("unknown project"),
        WorldOmission::ScopeDenied
    );
}

#[test]
fn m3_05_one_frame_becomes_one_untrusted_candidate() {
    let fixture = meeting_fixture();
    let service = fixture.service();
    let access = fixture.access();
    let ready = prepare(
        &service,
        &fixture,
        fixture.request(access, vec![fixture.meeting_ref(MEETING_ID)], None),
    )
    .unwrap_or_else(|omission| panic!("omitted {}", omission.as_str()));
    let candidate = ready.candidate();
    assert_eq!(candidate.source_kind, WORLD_SHADOW_KIND);
    assert_eq!(candidate.requirement, Requirement::May);
    assert_eq!(candidate.authority, Authority::UntrustedData);
    assert_eq!(candidate.placement, Placement::Base);
    assert_eq!(candidate.utility, 0);
    assert_eq!(candidate.source_version, 1);
    assert_eq!(candidate.cost_bytes, candidate.content.len());
    let digest = format!("{:x}", Sha256::digest(candidate.content.as_bytes()));
    assert_eq!(candidate.source_digest, digest);
    assert_eq!(
        candidate.source_id,
        format!("world-frame:{RUN_ID}:{digest}")
    );
    assert_eq!(candidate.candidate_id, candidate.source_id);
    assert!(candidate.scope_refs.contains(&fixture.project));
}

#[test]
fn m3_06_prepare_and_revalidate_use_one_service() {
    let fixture = Fixture::file(&[("resource", MEETING_ID)], 0, false);
    fixture.add_meeting(MEETING_ID, "active", "100", None, None);
    fixture.meeting.set(Some(MEETING_ID), MeetingState::Active);
    let service = fixture.service();
    let access = fixture.access();
    let ready = prepare(
        &service,
        &fixture,
        fixture.request(access, vec![fixture.meeting_ref(MEETING_ID)], None),
    )
    .unwrap_or_else(|omission| panic!("omitted {}", omission.as_str()));
    assert!(matches!(
        service.revalidate_frame(ready.prepared()).unwrap(),
        FrameValidity::Current
    ));
    assert!(matches!(
        fixture
            .service()
            .revalidate_frame(ready.prepared())
            .unwrap(),
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
    let ready = prepare(
        &service,
        &fixture,
        fixture.request(
            access,
            vec![fixture.meeting_ref(MEETING_ID)],
            Some(fixture.graph_request("ent0")),
        ),
    )
    .unwrap_or_else(|omission| panic!("omitted {}", omission.as_str()));
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
    let omission = match prepare(
        &service,
        &fixture,
        fixture.request(access, vec![fixture.meeting_ref(MEETING_ID)], None),
    ) {
        Err(omission) => omission,
        Ok(_) => panic!("expected denial"),
    };
    assert_eq!(omission, WorldOmission::ScopeDenied);
}

#[test]
fn m3_07_notice_only_frame_is_empty() {
    let fixture = Fixture::new(&[]);
    let access = fixture.access();
    reset_prepare_calls();
    let omission = match prepare(
        &fixture.service(),
        &fixture,
        fixture.request(access, Vec::new(), Some(fixture.graph_request("missing"))),
    ) {
        Err(omission) => omission,
        Ok(_) => panic!("expected empty"),
    };
    assert_eq!(omission, WorldOmission::EmptyFrame);
    assert_eq!(super::source::prepare_calls(), 1);
}
