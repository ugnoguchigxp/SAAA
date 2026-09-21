//! G1 integration tests: the fixed graph question reaches one authorized five-element slice and the
//! C3/C5 boundaries stay explicit. Deterministic fixtures only; no model, network or production DB.

use super::question::{parse_graph_question, QuestionParse};
use super::render::parse_rendered_json;
use super::source::{
    prepare_candidate, prepare_explicit_question_candidate, WorldOmission, WorldSourceOutcome,
    WorldSourceRequest, WORLD_KIND,
};
use super::turn::{compose_parts, TurnCompose};
use crate::memory::context_window::{ContextHealthReport, ContextWindow, ProjectedContextMessage};
use crate::memory::personal_state::world::query::WorldSeed;
use crate::memory::personal_state::world::query_v2::IncludeFlags;
use crate::memory::personal_state::world::runtime_frame::GraphRequest;
use crate::memory::personal_state::world::runtime_test_support::{Fixture, RUN_ID};
use crate::memory::personal_state::world::test_support::*;
use crate::runtime::context::scope::ScopeSnapshot;
use saaa_personal_state_core::world::model_v2::EntityKindV2;
use saaa_personal_state_core::world::traversal_v2::{CausalDirection, LimitsV2};
use saaa_personal_state_core::SourceRef;
use serde_json::Value;
use std::collections::BTreeSet;
use std::sync::Arc;

const TECH_QUESTION: &str = "「Speculative Decoding」は今の目標にどう関係しますか？";

pub(crate) fn window() -> ContextWindow {
    ContextWindow {
        messages: vec![
            ProjectedContextMessage {
                role: "system".into(),
                content: "policy".into(),
            },
            ProjectedContextMessage {
                role: "user".into(),
                content: TECH_QUESTION.into(),
            },
        ],
        continuity_groups: Vec::new(),
        health: ContextHealthReport {
            status: "green",
            hard_limit_bytes: 32_000,
            provider_capacity_bytes: 64_000,
            output_reserve_bytes: 20_000,
            safety_margin_bytes: 12_000,
            projected_bytes: 13,
            loaded_source_messages: 1,
            source_history_truncated: false,
            recent_source_messages: 0,
            continuity_group_count: 0,
            continuity_source_messages: 0,
            memory_item_count: 0,
            omitted_memory_items: 0,
            omitted_loaded_source_messages: 0,
            current_instruction_count: 1,
            repair_count: 0,
        },
    }
}

pub(crate) fn load_scope(fixture: &Fixture) -> ScopeSnapshot {
    fixture
        .writer
        .read_serialized(|connection| crate::runtime::context::scope::load(connection, RUN_ID))
        .expect("scope")
}

pub(crate) fn allowed(scope: &ScopeSnapshot) -> BTreeSet<String> {
    scope.scopes.iter().map(|item| item.key.clone()).collect()
}

pub(crate) fn graph_request(topic: &str) -> GraphRequest {
    let parse = parse_graph_question(&format!("「{topic}」は今の目標にどう関係しますか？"));
    parse.graph_request().expect("requested")
}

fn entity_graph_request(seed: &str) -> GraphRequest {
    GraphRequest {
        seeds: vec![WorldSeed::EntityId(seed.into())],
        causal_direction: CausalDirection::Forward,
        limits: LimitsV2::m1().capped(),
        flags: IncludeFlags::default(),
        explicit_question: true,
    }
}

fn omission_label(outcome: &WorldSourceOutcome) -> Option<WorldOmission> {
    match outcome {
        WorldSourceOutcome::Omitted(omission) => Some(*omission),
        WorldSourceOutcome::Ready(_) => None,
    }
}

fn prepare_outcome(fixture: &Fixture, request: GraphRequest, explicit: bool) -> WorldSourceOutcome {
    let access = fixture.access();
    let frame = fixture.request(access, Vec::new(), Some(request));
    if explicit {
        prepare_explicit_question_candidate(
            &fixture.service(),
            WorldSourceRequest {
                frame_request: frame,
            },
            &load_scope(fixture),
        )
    } else {
        prepare_candidate(
            &fixture.service(),
            WorldSourceRequest {
                frame_request: frame,
            },
            &load_scope(fixture),
        )
    }
}

fn commit_one(fixture: &Fixture, fence: &str, built: Vec<BuiltAssertion>) {
    let mut committer = Committer {
        writer: fixture.writer.as_ref(),
        project: PROJECT,
    };
    committer
        .commit(fence, built)
        .unwrap_or_else(|error| panic!("{fence}: {error}"));
}

fn commit_entities(fixture: &Fixture, source: &SourceRef, now_ms: i64) {
    let entities = [
        ("g1-ent-p", "ent_p", "p", EntityKindV2::Project, "SAAA"),
        (
            "g1-ent-tech",
            "ent_tech",
            "tech",
            EntityKindV2::Concept,
            "Speculative Decoding",
        ),
        (
            "g1-ent-decode",
            "ent_decode",
            "decode",
            EntityKindV2::Metric,
            "Decode Latency",
        ),
        (
            "g1-ent-voice",
            "ent_voice",
            "voice",
            EntityKindV2::Metric,
            "Voice Latency",
        ),
        (
            "g1-ent-goal",
            "ent_goal",
            "goal",
            EntityKindV2::Goal,
            "Natural Conversation",
        ),
        (
            "g1-ent-user",
            "ent_user",
            "user",
            EntityKindV2::Actor,
            "User",
        ),
    ];
    for (fence, id, entity_id, kind, name) in entities {
        let objective = (entity_id == "goal").then_some("g1-obj");
        let aliases: &[&str] = if entity_id == "tech" {
            &["tech", "spec-dec"]
        } else {
            &[]
        };
        commit_one(
            fixture,
            fence,
            vec![v2_entity_assertion(
                id, entity_id, kind, name, aliases, objective, source, PROJECT, now_ms,
            )],
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn relation_payload(
    from: &str,
    to: &str,
    relation_type: &str,
    effect_input: Option<&str>,
    target_direction: Option<&str>,
    correlation_sign: Option<&str>,
    signed: bool,
    key: &saaa_personal_state_core::SourceKey,
) -> Value {
    let (comparison, confidence) = if signed {
        (Some("cmp"), Some((800, "manual_v1")))
    } else {
        (None, None)
    };
    v2_relation_value(
        from,
        to,
        relation_type,
        effect_input,
        &[("config", "a")],
        comparison,
        target_direction,
        correlation_sign,
        confidence,
        std::slice::from_ref(key),
        None,
        None,
    )
}

fn commit_relations(fixture: &Fixture, source: &SourceRef, now_ms: i64) {
    let key = source.key.clone();
    let depends =
        |ids: &[&str]| -> BTreeSet<String> { ids.iter().map(|id| id.to_string()).collect() };
    let relations = [
        (
            "g1-rel-tech-decode",
            "rel_tech_decode",
            relation_payload(
                "tech",
                "decode",
                "increases",
                Some("intervention"),
                None,
                None,
                true,
                &key,
            ),
            &["ent_tech", "ent_decode"][..],
        ),
        (
            "g1-rel-decode-voice",
            "rel_decode_voice",
            relation_payload(
                "decode",
                "voice",
                "increases",
                Some("quantity_increase"),
                None,
                None,
                true,
                &key,
            ),
            &["ent_decode", "ent_voice"][..],
        ),
        (
            "g1-rel-voice-goal",
            "rel_voice_goal",
            relation_payload(
                "voice",
                "goal",
                "serves_goal",
                None,
                Some("lower_is_better"),
                None,
                false,
                &key,
            ),
            &["ent_voice", "ent_goal"][..],
        ),
        (
            "g1-rel-p-goal",
            "rel_p_goal",
            relation_payload("p", "goal", "has_goal", None, None, None, false, &key),
            &["ent_p", "ent_goal"][..],
        ),
        (
            "g1-rel-voice-decode",
            "rel_voice_decode",
            relation_payload(
                "voice",
                "decode",
                "correlates_with",
                None,
                None,
                Some("positive"),
                false,
                &key,
            ),
            &["ent_voice", "ent_decode"][..],
        ),
        (
            "g1-rel-decode-tech",
            "rel_decode_tech",
            relation_payload(
                "decode",
                "tech",
                "depends_on",
                None,
                None,
                None,
                false,
                &key,
            ),
            &["ent_decode", "ent_tech"][..],
        ),
    ];
    for (fence, id, payload, deps) in relations {
        commit_one(
            fixture,
            fence,
            vec![v2_relation_assertion(
                id,
                payload,
                source,
                PROJECT,
                depends(deps),
                now_ms,
            )],
        );
    }
    commit_one(
        fixture,
        "g1-focus",
        vec![v2_focus_assertion(
            "focus_goal",
            "goal",
            "current_work",
            Some("g1-obj"),
            source,
            PROJECT,
            depends(&["ent_goal", "g1-obj"]),
            now_ms,
        )],
    );
}

/// The five-element fixture from the plan: project:p, concept:tech, metric:decode, metric:voice,
/// goal:natural and the tech→decode→voice, voice→goal and project→goal edges plus one correlation
/// and one depends_on.
pub(crate) fn g1_fixture() -> Fixture {
    let fixture = Fixture::new(&[]);
    let now_ms = crate::memory::personal_state::now();
    let source = fixture
        .writer
        .write(|c| Ok(insert_source(c, PROJECT, "g1-src", "g1 graph source")))
        .expect("source");
    commit_one(
        &fixture,
        "g1-objective",
        vec![objective_assertion(
            "g1-obj", "g1-obj-p", &source, PROJECT, now_ms,
        )],
    );
    commit_entities(&fixture, &source, now_ms);
    commit_relations(&fixture, &source, now_ms);
    fixture
        .writer
        .write(|c| {
            c.execute(
                "UPDATE personal_jobs SET status='completed' WHERE source_sequence=(
                   SELECT sequence FROM personal_sources WHERE message_id='g1-src')",
                [],
            )
            .map_err(crate::database_error)?;
            Ok(())
        })
        .expect("complete job");
    // The frame service uses the fixture clock; align it with the commit time so the fresh
    // projection is within the entity validity window.
    fixture.set_now(now_ms);
    fixture
}

fn world_candidate(composed: &TurnCompose) -> Option<&crate::runtime::context::source::Candidate> {
    composed
        .envelope
        .selected
        .iter()
        .find(|candidate| candidate.source_kind == WORLD_KIND)
}

#[test]
fn world_g1_03_explicit_question_accepts_one_exact_name_but_shadow_does_not() {
    let fixture = g1_fixture();
    let outcome = prepare_outcome(&fixture, graph_request("Speculative Decoding"), true);
    assert!(
        matches!(outcome, WorldSourceOutcome::Ready(_)),
        "explicit question must accept ExactName: {:?}",
        omission_label(&outcome)
    );
    let outcome = prepare_outcome(&fixture, graph_request("Speculative Decoding"), false);
    match outcome {
        WorldSourceOutcome::Omitted(WorldOmission::InvalidInput) => {}
        other => panic!(
            "shadow must refuse ExactName, got omission {:?}",
            omission_label(&other)
        ),
    }
}

#[test]
fn world_g1_05_unknown_seed_renders_an_empty_frame_with_the_fixed_notice() {
    let fixture = g1_fixture();
    let outcome = prepare_outcome(&fixture, graph_request("missing"), true);
    let ready = match outcome {
        WorldSourceOutcome::Ready(ready) => ready,
        other => panic!(
            "unknown seed must render, got omission {:?}",
            omission_label(&other)
        ),
    };
    let json = parse_rendered_json(&ready.candidate().content);
    assert_eq!(json["graph"]["nodes"].as_array().map(Vec::len), Some(0));
    assert!(json["graph"]["notices"]
        .as_array()
        .unwrap()
        .iter()
        .any(|notice| notice == "unknown_seed"));
}

#[test]
fn world_g1_05_ambiguous_seed_is_not_auto_selected() {
    let fixture = g1_fixture();
    let now_ms = crate::memory::personal_state::now();
    let source = fixture
        .writer
        .write(|c| Ok(insert_source(c, PROJECT, "g1-amb", "ambiguous source")))
        .expect("source");
    commit_one(
        &fixture,
        "g1-amb-a",
        vec![v2_entity_assertion(
            "ent_dup_a",
            "dup-a",
            EntityKindV2::Concept,
            "Duplicate",
            &[],
            None,
            &source,
            PROJECT,
            now_ms,
        )],
    );
    commit_one(
        &fixture,
        "g1-amb-b",
        vec![v2_entity_assertion(
            "ent_dup_b",
            "dup-b",
            EntityKindV2::Concept,
            "Duplicate",
            &[],
            None,
            &source,
            PROJECT,
            now_ms,
        )],
    );
    fixture.set_now(now_ms);
    let outcome = prepare_outcome(&fixture, graph_request("Duplicate"), true);
    let ready = match outcome {
        WorldSourceOutcome::Ready(ready) => ready,
        other => panic!(
            "ambiguous seed must render, got omission {:?}",
            omission_label(&other)
        ),
    };
    let json = parse_rendered_json(&ready.candidate().content);
    assert!(json["graph"]["notices"]
        .as_array()
        .unwrap()
        .iter()
        .any(|notice| notice == "ambiguous_seed"));
    let names = json["graph"]["nodes"]
        .as_array()
        .map(|nodes| {
            nodes
                .iter()
                .filter_map(|node| node["name"].as_str())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    assert!(!names.contains(&"Duplicate"));
}

#[test]
fn world_g1_05_projection_stale_is_rendered_but_shadow_stays_empty() {
    let fixture = Fixture::new(&[]);
    let explicit = prepare_outcome(&fixture, graph_request("Speculative Decoding"), true);
    let ready = match explicit {
        WorldSourceOutcome::Ready(ready) => ready,
        other => panic!(
            "stale projection must render, got omission {:?}",
            omission_label(&other)
        ),
    };
    let json = parse_rendered_json(&ready.candidate().content);
    assert!(json["notices"]
        .as_array()
        .unwrap()
        .iter()
        .any(|notice| notice["code"] == "world_projection_stale"));
    let shadow = prepare_outcome(&fixture, entity_graph_request("missing"), false);
    match shadow {
        WorldSourceOutcome::Omitted(WorldOmission::EmptyFrame) => {}
        other => panic!(
            "shadow notice-only frame must be empty, got omission {:?}",
            omission_label(&other)
        ),
    }
}

#[test]
fn world_g1_09_notice_only_frame_reaches_the_envelope_block() {
    // A stale projection is not a successful empty knowledge; the fixed notice must be visible to
    // the provider through the same broker block.
    let fixture = Fixture::new(&[]);
    let scope = load_scope(&fixture);
    let composed = compose_parts(
        true,
        Some(Arc::new(fixture.service())),
        fixture.access().principal,
        fixture.access().policy_revision,
        Some(graph_request("tech")),
        RUN_ID,
        &scope,
        window(),
        Vec::new(),
        allowed(&scope),
    )
    .expect("compose");
    let candidate = world_candidate(&composed).expect("world candidate");
    let combined = composed
        .envelope
        .combined_block
        .as_deref()
        .expect("combined block");
    assert!(combined.contains(&candidate.content));
    assert!(combined.contains("world_projection_stale"), "{combined}");
}

#[test]
fn world_g1_04_graph_only_question_fetches_a_frame_without_runtime_refs() {
    let fixture = g1_fixture();
    let scope = load_scope(&fixture);
    let composed = compose_parts(
        true,
        Some(Arc::new(fixture.service())),
        fixture.access().principal,
        fixture.access().policy_revision,
        Some(graph_request("Speculative Decoding")),
        RUN_ID,
        &scope,
        window(),
        Vec::new(),
        allowed(&scope),
    )
    .expect("compose");
    let candidate = world_candidate(&composed).expect("world candidate");
    // The combined block is exactly what the broker inserts before the current instruction and the
    // provider history/body is rendered from; the graph JSON must survive verbatim.
    let combined = composed
        .envelope
        .combined_block
        .as_deref()
        .expect("combined block");
    assert!(combined.contains(&candidate.content));
    assert!(combined.contains("\"nodes\""));
    assert!(combined.contains("\"notices\""));
    let json = parse_rendered_json(&candidate.content);
    assert!(json["runtime"].as_array().unwrap().is_empty());
    let names = json["graph"]["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|node| node["name"].as_str())
        .collect::<Vec<_>>();
    assert!(
        names.contains(&"Speculative Decoding"),
        "names={names:?} json={json}"
    );
    assert!(
        names.contains(&"Decode Latency"),
        "names={names:?} json={json}"
    );
    assert!(
        names.contains(&"Voice Latency"),
        "names={names:?} json={json}"
    );
    let relation_types = json["graph"]["relations"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|relation| relation["relation_type"].as_str())
        .collect::<Vec<_>>();
    assert!(
        relation_types.contains(&"correlates_with"),
        "{relation_types:?}"
    );
    assert!(relation_types.contains(&"depends_on"), "{relation_types:?}");
}

#[test]
fn world_g1_06_alias_resolves_only_when_it_is_unique() {
    let fixture = g1_fixture();
    let outcome = prepare_outcome(&fixture, graph_request("spec-dec"), true);
    let ready = match outcome {
        WorldSourceOutcome::Ready(ready) => ready,
        other => panic!(
            "unique alias must resolve, got omission {:?}",
            omission_label(&other)
        ),
    };
    let json = parse_rendered_json(&ready.candidate().content);
    let names = json["graph"]["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|node| node["name"].as_str())
        .collect::<Vec<_>>();
    assert!(names.contains(&"Speculative Decoding"), "{names:?}");
}

#[test]
fn world_g1_07_plain_chat_keeps_the_runtime_only_path() {
    let fixture = g1_fixture();
    let scope = load_scope(&fixture);
    let composed = compose_parts(
        true,
        Some(Arc::new(fixture.service())),
        fixture.access().principal,
        fixture.access().policy_revision,
        None,
        RUN_ID,
        &scope,
        window(),
        Vec::new(),
        allowed(&scope),
    )
    .expect("compose");
    assert!(world_candidate(&composed).is_none());
    assert!(composed.world.is_none());
}

#[test]
fn world_g1_01_not_requested_never_builds_a_graph_request() {
    assert!(parse_graph_question(TECH_QUESTION).is_requested());
    assert_eq!(
        parse_graph_question("普通の雑談です"),
        QuestionParse::NotRequested
    );
    assert_eq!(
        parse_graph_question(TECH_QUESTION)
            .graph_request()
            .unwrap()
            .seeds,
        vec![WorldSeed::ExactName("Speculative Decoding".into())]
    );
    assert_eq!(
        graph_request("Speculative Decoding").limits,
        LimitsV2::m1().capped()
    );
    assert_eq!(
        graph_request("Speculative Decoding").causal_direction,
        CausalDirection::Forward
    );
    assert_eq!(
        graph_request("Speculative Decoding").flags,
        IncludeFlags::default()
    );
}
