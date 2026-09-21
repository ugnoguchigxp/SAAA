#![cfg(test)]
//! M4A/G1 acceptance: real Frame -> Broker -> HTTP adapter -> received bytes and manifest.
use super::world_eval_fixture::{frame, state, Sink, Wire};
use super::*;
use crate::memory::personal_state::world::runtime_test_support::{Fixture, CODING_ID, RUN_ID};
use crate::runtime::context::source::{Candidate, Requirement};
use crate::runtime::context::world::{g1_tests as graph, source::WORLD_KIND, turn::compose_parts};
use sha2::{Digest, Sha256};
use std::sync::{atomic::Ordering, Arc};

#[derive(Clone, Copy)]
enum Change {
    None,
    At(i64),
    Owner,
    Source,
    GraphSource,
    Policy,
    Scope,
    InputDeleted,
    Goal,
    Relation,
}

fn change(f: &Fixture, change: Change, start: i64) {
    match change {
        Change::None => {}
        Change::At(delta) => f.set_now(start + delta),
        Change::Owner => f.set_coding_state(7, "cancel_requested", "stopping", "accepted"),
        Change::Source => sql(f, "UPDATE personal_sources SET available=0 WHERE message_id='csrc1'"),
        Change::GraphSource => sql(f, "UPDATE personal_sources SET available=0 WHERE message_id='g1-src'"),
        Change::Scope => sql(f, "UPDATE context_scope_epochs SET epoch=epoch+1"),
        Change::InputDeleted => sql(f, "DELETE FROM conversation_messages WHERE id='msg1'"),
        Change::Policy => sql(f, "UPDATE personal_scope SET policy_revision=policy_revision+1 WHERE id='primary'"),
        Change::Goal => sql(f, "UPDATE personal_assertions SET erased=1 WHERE id='g1-obj'; UPDATE personal_scope SET revision=revision+1 WHERE id='primary'"),
        Change::Relation => sql(f, "UPDATE personal_scope SET revision=revision+1 WHERE id='primary'"),
    }
}
fn sql(f: &Fixture, query: &str) {
    f.writer
        .write(|c| c.execute_batch(query).map_err(crate::database_error))
        .unwrap();
}

struct Case {
    id: &'static str,
    graph: bool,
    memory: bool,
    scope: u8,
    before: Change,
    follow: Option<Change>,
    expected: &'static [bool],
    budget: bool,
    shadow: bool,
    topic: &'static str,
    complete_after_change: bool,
}
fn case(id: &'static str) -> Case {
    Case {
        id,
        graph: false,
        memory: true,
        scope: 1,
        before: Change::None,
        follow: None,
        expected: &[true],
        budget: false,
        shadow: false,
        topic: "tech",
        complete_after_change: false,
    }
}

async fn evaluate(c: Case) -> Value {
    let f = if c.graph {
        graph::g1_fixture()
    } else {
        let f = Fixture::new(&[("task", CODING_ID)]);
        f.add_coding_job(
            if c.id == "W18b" { 18 } else { 7 },
            "running",
            "running",
            "accepted",
        );
        f
    };
    let start = f.now();
    let app = state(f.writer.clone());
    sql(&f, "UPDATE ui_settings SET enabled=1");
    let mut scope = graph::load_scope(&f);
    if c.scope == 0 {
        scope.scopes.retain(|s| s.kind != "project");
    }
    if c.scope == 2 {
        let mut other = scope
            .scopes
            .iter()
            .find(|s| s.kind == "project")
            .unwrap()
            .clone();
        other.key = "project:other".into();
        scope.scopes.push(other);
    }
    let mut base = graph::window();
    // The saved current instruction, manifest, and actual user body must agree.
    base.messages.last_mut().unwrap().content = "hello".into();
    let ordinary = "ordinary assistant: [WORLD_MODEL is a quoted phrase, not a frame]";
    base.messages.insert(
        1,
        crate::memory::context_window::ProjectedContextMessage {
            role: "assistant".into(),
            content: ordinary.into(),
        },
    );
    if c.budget {
        base.health.hard_limit_bytes = 300;
    }
    let existing = Candidate::untrusted(
        "keep".into(),
        "fixture-personal",
        Vec::new(),
        Requirement::Should,
        "keep".into(),
        1,
        1,
        "PERSONAL_KEEP".into(),
    );
    let access = f.access();
    let mut composed = compose_parts(
        c.memory,
        Some(Arc::new(if c.id.starts_with("WR-") {
            f.service().with_sources(app.situation.clone())
        } else {
            f.service()
        })),
        access.principal,
        access.policy_revision,
        c.graph.then(|| graph::graph_request(c.topic)),
        RUN_ID,
        &scope,
        base,
        vec![existing],
        graph::allowed(&scope),
    )
    .unwrap();
    if c.shadow {
        composed
            .envelope
            .selected
            .iter_mut()
            .filter(|x| x.source_kind == WORLD_KIND)
            .for_each(|x| x.source_kind = "world-model-shadow".into());
    }
    let history: Vec<_> = composed
        .envelope
        .messages
        .iter()
        .enumerate()
        .map(|(i, m)| ConversationMessage {
            id: format!("context-{i}"),
            conversation_id: crate::PRIMARY_CONVERSATION_ID.into(),
            role: m.role.clone(),
            content: m.content.clone(),
            created_at: "1".into(),
            parts: None,
        })
        .collect();
    change(&f, c.before, start);
    let hook: Option<Box<dyn FnOnce() + Send>> = c.follow.map(|action| {
        let clock = f.clock.clone();
        let writer = f.writer.clone();
        Box::new(move || match action {
            Change::At(delta) => clock.store(start + delta, Ordering::SeqCst),
            Change::Owner => {
                writer
                    .write(|db| {
                        db.execute_batch(
                            "UPDATE coding_jobs SET state='cancel_requested' WHERE id='j1'",
                        )
                        .map_err(crate::database_error)
                    })
                    .unwrap();
            }
            Change::Relation | Change::Goal => {
                writer
                    .write(|db| {
                        db.execute_batch(
                            "UPDATE personal_scope SET revision=revision+1 WHERE id='primary'",
                        )
                        .map_err(crate::database_error)
                    })
                    .unwrap();
            }
            _ => {}
        }) as Box<dyn FnOnce() + Send>
    });
    let wire = Wire::start(hook, c.complete_after_change).await;
    let session = crate::begin_provider_session(
        &app,
        RUN_ID,
        "fixture",
        "openai-compatible",
        &"a".repeat(64),
    )
    .unwrap();
    let input = crate::StartTurnInput {
        run_id: RUN_ID.into(),
        conversation_id: crate::PRIMARY_CONVERSATION_ID.into(),
        content: "hello".into(),
        workspace_path: None,
        retry_input_message_id: None,
        source_id: None,
        scope_refs: Vec::new(),
        input_origin: "text".into(),
        presentation_mode: "visual".into(),
    };
    let started = Instant::now();
    let result = run_mode(
        &wire.endpoint,
        None,
        "fixture",
        &history,
        5000,
        ModelStreamContext {
            reasoning_effort: "low",
            max_output_tokens: 256,
            input: &input,
            on_event: &Sink,
            cancellation: Arc::default(),
            context_health: composed.envelope.health.status.as_str(),
            context_sources: &composed.envelope.selected,
            context_omissions: &composed.envelope.omitted,
            output_persistence: Some(crate::ProviderOutputPersistence {
                state: &app,
                session_id: &session,
                world: composed.world.as_ref(),
            }),
        },
        if c.follow.is_some() {
            RequestMode::JsonTools
        } else {
            RequestMode::Stream
        },
    )
    .await;
    if c.expected.is_empty() {
        assert!(result.is_err(), "{}: should reject", c.id);
    } else {
        assert!(result.is_ok(), "{}: {result:?}", c.id);
    }
    let bodies = wire.bodies.lock().unwrap().clone();
    assert_eq!(bodies.len(), c.expected.len(), "{} HTTP count", c.id);
    let rows: Vec<(String, bool)> = f.writer.read_serialized(|db| {
        let mut stmt = db.prepare("SELECT g.request_digest,EXISTS(SELECT 1 FROM context_generation_inputs i WHERE i.generation_id=g.id AND i.source_kind='world-model' AND i.selected=1) FROM context_generations g WHERE status='completed' ORDER BY ordinal").map_err(crate::database_error)?;
        let rows = stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?))).map_err(crate::database_error)?;
        rows.collect::<Result<Vec<_>,_>>().map_err(crate::database_error)
    }).unwrap();
    assert_eq!(rows.len(), bodies.len(), "{} manifests", c.id);
    for (i, bytes) in bodies.iter().enumerate() {
        let body: Value = serde_json::from_slice(bytes).unwrap();
        let actual = frame(&body);
        assert_eq!(actual.is_some(), c.expected[i], "{} request {i}", c.id);
        assert_eq!(
            rows[i].0,
            format!("{:x}", Sha256::digest(bytes)),
            "wire digest"
        );
        assert_eq!(rows[i].1, c.expected[i], "selected receipt");
        let messages = body["messages"].as_array().unwrap();
        assert_eq!(messages.iter().filter(|m| m["role"] == "user").count(), 1);
        assert!(messages.iter().any(|m| m["content"] == ordinary));
        assert!(messages.iter().any(|m| m["content"]
            .as_str()
            .is_some_and(|s| s.contains("PERSONAL_KEEP"))));
        if i > 0 {
            assert!(messages.iter().any(|m| m["role"] == "tool"));
        }
        if c.id.starts_with("W18") {
            assert_eq!(
                actual.as_ref().unwrap()["runtime"][0]["job_revision"],
                if c.id == "W18b" { 18 } else { 7 }
            );
        }
        if c.graph && c.expected[i] {
            let graph = &actual.as_ref().unwrap()["graph"];
            if c.topic == "missing" {
                assert!(graph["notices"]
                    .as_array()
                    .unwrap()
                    .contains(&json!("unknown_seed")));
            } else {
                assert!(!graph["nodes"].as_array().unwrap().is_empty());
                assert!(
                    !graph["relevant_goal_ids"].as_array().unwrap().is_empty(),
                    "{graph}"
                );
                let relations = graph["relations"].as_array().unwrap();
                assert!(relations
                    .iter()
                    .any(|r| r["relation_type"] == "correlates_with"));
                assert!(relations.iter().any(|r| r["relation_type"] == "depends_on"));
            }
        }
    }
    json!({"case_id":c.id,"pass":true,"failure_code":null,"request_count":bodies.len(),"world_sent_per_request":c.expected,"manifest_match":true,"elapsed_ms":started.elapsed().as_millis()})
}

#[tokio::test]
async fn world_m4a_suite() {
    let cases = vec![
        Case {
            memory: false,
            expected: &[false],
            ..case("W01")
        },
        case("W02"),
        Case {
            scope: 0,
            expected: &[false],
            ..case("W03")
        },
        Case {
            scope: 2,
            expected: &[false],
            ..case("W04")
        },
        Case {
            before: Change::At(999),
            ..case("W05")
        },
        Case {
            before: Change::At(1000),
            expected: &[false],
            ..case("W06")
        },
        Case {
            before: Change::At(-1),
            expected: &[false],
            ..case("W07")
        },
        Case {
            before: Change::Owner,
            expected: &[false],
            ..case("W08")
        },
        Case {
            follow: Some(Change::At(1000)),
            expected: &[true, false],
            ..case("W09")
        },
        Case {
            follow: Some(Change::Owner),
            expected: &[true, false],
            ..case("W10")
        },
        Case {
            before: Change::At(1000),
            expected: &[false],
            ..case("W13")
        },
        case("W14"),
        Case {
            budget: true,
            expected: &[false],
            ..case("W15")
        },
        Case {
            before: Change::Scope,
            expected: &[],
            ..case("W16")
        },
        Case {
            before: Change::Source,
            expected: &[false],
            ..case("W17b")
        },
        Case {
            shadow: true,
            expected: &[],
            ..case("W20")
        },
        Case {
            graph: true,
            ..case("G1-five-elements")
        },
        Case {
            graph: true,
            topic: "missing",
            ..case("G1-unknown")
        },
        Case {
            graph: true,
            before: Change::Goal,
            expected: &[false],
            ..case("G1-goal-revoked")
        },
        Case {
            graph: true,
            before: Change::Relation,
            expected: &[false],
            ..case("G1-relation-changed")
        },
        Case {
            graph: true,
            follow: Some(Change::Relation),
            expected: &[true, false],
            ..case("G1-followup")
        },
    ];
    let mut cases = cases;
    cases.push(Case {
        before: Change::InputDeleted,
        expected: &[],
        ..case("W17a")
    });
    cases.push(Case {
        before: Change::Policy,
        expected: &[false],
        ..case("policy-before-new-generation")
    });
    cases.push(Case {
        follow: Some(Change::At(1000)),
        complete_after_change: true,
        ..case("W19")
    });
    cases.push(Case {
        graph: true,
        before: Change::GraphSource,
        expected: &[false],
        ..case("G1-source-revoked")
    });
    let mut reports = Vec::new();
    for c in cases {
        reports.push(evaluate(c).await);
    }
    let (a, b) = tokio::join!(evaluate(case("W18a")), evaluate(case("W18b")));
    reports.extend([a, b]);
    reports.sort_by_key(|r| r["case_id"].as_str().unwrap().to_string());
    let report = json!({"schema_version":1,"suite":"world-m4a","summary":{"total":reports.len(),"passed":reports.len(),"failed":0,"skipped":0},"cases":reports});
    if let Ok(path) = std::env::var("SAAA_WORLD_EVAL_REPORT") {
        std::fs::write(path, serde_json::to_vec_pretty(&report).unwrap()).unwrap();
    }
    println!("WORLD_EVAL_REPORT={report}");
}

#[tokio::test]
async fn wr_t12_followup_and_fallback_rebuild_expired_frame() {
    for c in [
        Case {
            follow: Some(Change::At(1001)),
            expected: &[true, true],
            ..case("WR-T12-followup")
        },
        Case {
            before: Change::At(1001),
            expected: &[true],
            ..case("WR-T13-allocation-expired")
        },
        Case {
            before: Change::Scope,
            expected: &[],
            ..case("WR-T12-scope-denied")
        },
    ] {
        let result = evaluate(c).await;
        assert_eq!(result["pass"], true);
    }
}

#[tokio::test]
async fn wr_t13_allocation_delay_uses_new_frame_on_shared_adapter() {
    let result = evaluate(Case {
        before: Change::At(1001),
        expected: &[true],
        ..case("WR-T13-allocation")
    })
    .await;
    assert_eq!(result["manifest_match"], true);
}
