use super::delivery::{digest_for, finalize, Terminal};
use super::test_fakes::*;
use super::{cancel_for_input, compose_system, JsonRunner};
use crate::worker_agents::contracts::*;
use crate::worker_agents::loader::{load_revision, now_ms};
use rusqlite::params;
use std::sync::Arc;
use std::time::Duration;

fn code_of(outcome: &WorkerOutcome) -> FailureCode {
    match outcome {
        WorkerOutcome::Failed { failure, .. } => failure.code,
        other => panic!("expected a failure, got {other:?}"),
    }
}

async fn settle(harness: &Harness, task_id: &str) -> WorkerOutcome {
    harness.run_next().await.unwrap();
    harness.exec.wait_terminal(task_id, Duration::ZERO).await
}

fn invalid() -> Result<WorkerOutput, AttemptError> {
    Err(AttemptError::InvalidOutput("bad".into()))
}

#[tokio::test]
async fn malformed_model_output_once_is_retried_on_the_same_tier() {
    let mut setup = Setup::new();
    setup.model = FakeModel::new(
        vec![route(Tier::Local, "l0")],
        vec![Ok("not json".into()), Ok(r#"{"answer": 1}"#.into())],
    );
    let model = setup.model.clone();
    let harness = Harness::build(setup);
    let task_id = harness.admit_task_id();
    let outcome = settle(&harness, &task_id).await;
    assert_eq!(
        outcome,
        WorkerOutcome::Succeeded {
            task_id: task_id.clone(),
            output: WorkerOutput::JsonV1(serde_json::json!({"answer": 1}))
        }
    );
    assert_eq!(model.fingerprints(), ["l0", "l0"]);
    let (system, input) = {
        let calls = model.calls.lock().unwrap();
        (calls[0].1.clone(), calls[0].2.clone())
    };
    assert!(system.contains("You are a narrow worker."));
    assert_eq!(input, r#"{"query":"桜"}"#);
    assert_eq!(
        harness.count("SELECT count(*) FROM worker_attempts WHERE status='failed'"),
        1
    );
    assert_eq!(harness.count("SELECT attempts FROM worker_tasks"), 2);
}

#[tokio::test]
async fn exhausted_retries_climb_to_the_next_tier() {
    let json = ScriptRunner::new(vec![invalid(), invalid(), json_ok()]);
    let mut setup = Setup::new();
    setup.draft.tier_policy.max_tier = Tier::LocalLarge;
    setup.model = FakeModel::new(
        vec![route(Tier::Local, "l0"), route(Tier::LocalLarge, "l1")],
        vec![],
    );
    setup.json = json.clone();
    let harness = Harness::build(setup);
    let task_id = harness.admit_task_id();
    let outcome = settle(&harness, &task_id).await;
    assert!(matches!(outcome, WorkerOutcome::Succeeded { .. }));
    assert_eq!(*json.fingerprints.lock().unwrap(), ["l0", "l0", "l1"]);
    assert_eq!(harness.task(&task_id).tier_index, 1);
    assert_eq!(harness.count("SELECT count(*) FROM worker_attempts"), 3);
}

#[tokio::test]
async fn tiers_above_the_profile_maximum_are_never_used() {
    let json = ScriptRunner::new(vec![invalid(), invalid(), json_ok()]);
    let mut setup = Setup::new();
    setup.model = FakeModel::new(
        vec![route(Tier::Local, "l0"), route(Tier::LocalLarge, "l1")],
        vec![],
    );
    setup.json = json.clone();
    let harness = Harness::build(setup);
    let task_id = harness.admit_task_id();
    let outcome = settle(&harness, &task_id).await;
    assert_eq!(code_of(&outcome), FailureCode::InvalidOutput);
    assert_eq!(*json.fingerprints.lock().unwrap(), ["l0", "l0"]);
}

fn cloud_setup(policy: CloudPolicy) -> (Setup, Arc<FakeModel>) {
    let mut setup = Setup::new();
    setup.draft.tier_policy = TierPolicy {
        max_tier: Tier::Cloud,
        cloud: policy,
    };
    setup.model = FakeModel::new(
        vec![route(Tier::Local, "l0"), route(Tier::Cloud, "c0")],
        vec![
            Ok("nope".into()),
            Ok("still nope".into()),
            Ok(r#"{"a":1}"#.into()),
        ],
    );
    let model = setup.model.clone();
    (setup, model)
}

#[tokio::test]
async fn a_cloud_rung_is_proposed_never_called() {
    let (setup, model) = cloud_setup(CloudPolicy::RequireApproval);
    let harness = Harness::build(setup);
    let task_id = harness.admit_task_id();
    let outcome = settle(&harness, &task_id).await;
    assert_eq!(code_of(&outcome), FailureCode::EscalationRequiresApproval);
    assert_eq!(
        model.fingerprints(),
        ["l0", "l0"],
        "no model call on the cloud rung"
    );
    assert_eq!(
        harness.count(
            "SELECT count(*) FROM worker_escalations WHERE status='proposed' AND tier='cloud'"
        ),
        1
    );
}

#[tokio::test]
async fn a_cloud_rung_is_not_even_proposed_when_the_profile_forbids_cloud() {
    let (setup, model) = cloud_setup(CloudPolicy::Never);
    let harness = Harness::build(setup);
    let task_id = harness.admit_task_id();
    let outcome = settle(&harness, &task_id).await;
    assert_eq!(code_of(&outcome), FailureCode::InvalidOutput);
    assert_eq!(model.fingerprints(), ["l0", "l0"]);
    assert_eq!(harness.count("SELECT count(*) FROM worker_escalations"), 0);
}

#[tokio::test]
async fn a_terminal_failure_is_not_escalated_or_retried() {
    let json = ScriptRunner::new(vec![Err(AttemptError::Terminal(
        FailureCode::NoSafeSources,
    ))]);
    let mut setup = Setup::new();
    setup.draft.tier_policy.max_tier = Tier::LocalLarge;
    setup.model = FakeModel::new(
        vec![route(Tier::Local, "l0"), route(Tier::LocalLarge, "l1")],
        vec![],
    );
    setup.json = json.clone();
    let harness = Harness::build(setup);
    let task_id = harness.admit_task_id();
    let outcome = settle(&harness, &task_id).await;
    assert_eq!(code_of(&outcome), FailureCode::NoSafeSources);
    assert_eq!(json.calls(), 1);
    assert_eq!(
        harness.task(&task_id).failure_code.as_deref(),
        Some("no_safe_sources")
    );
}

#[tokio::test]
async fn the_host_rechecks_the_output_against_the_schema() {
    let json = ScriptRunner::new(vec![
        Ok(WorkerOutput::JsonV1(serde_json::json!({"b": 1}))),
        Ok(WorkerOutput::JsonV1(serde_json::json!({"b": 2}))),
    ]);
    let mut setup = Setup::new();
    setup.draft.output_schema = Some(serde_json::json!({"type":"object","required":["a"]}));
    setup.json = json.clone();
    let harness = Harness::build(setup);
    let task_id = harness.admit_task_id();
    let outcome = settle(&harness, &task_id).await;
    assert_eq!(code_of(&outcome), FailureCode::CompletionUnmet);
    assert_eq!(json.calls(), 2);
}

fn web_setup(record_source: bool) -> (Setup, Arc<ScriptRunner>) {
    let web = ScriptRunner::with_hook(
        vec![Ok(claims_output("https://example.com/sakura")); 2],
        move |env| {
            if record_source {
                env.writer
                    .transact(|c| {
                        c.execute(
                            "INSERT OR IGNORE INTO worker_sources(task_id, url, host, kind, status,
                                created_at_ms) VALUES(?1, ?2, 'example.com', 'fetched', 'usable', 1)",
                            params![env.task_id, "https://example.com/sakura"],
                        )
                        .map(|_| ())
                        .map_err(|e| e.to_string())
                    })
                    .unwrap();
            }
        },
    );
    let mut setup = Setup::new();
    setup.draft.output_kind = OutputKind::WebClaimsV1;
    setup.draft.output_schema = None;
    setup.draft.completion = CompletionCriteria {
        min_items: 1,
        sources_must_be_host_recorded: true,
    };
    setup.web = web.clone();
    (setup, web)
}

#[tokio::test]
async fn web_claims_must_cite_sources_the_host_recorded() {
    let (setup, web) = web_setup(false);
    let harness = Harness::build(setup);
    let task_id = harness.admit_task_id();
    assert_eq!(
        code_of(&settle(&harness, &task_id).await),
        FailureCode::CompletionUnmet
    );
    assert_eq!(web.calls(), 2);

    let (setup, _) = web_setup(true);
    let harness = Harness::build(setup);
    let task_id = harness.admit_task_id();
    assert!(matches!(
        settle(&harness, &task_id).await,
        WorkerOutcome::Succeeded {
            output: WorkerOutput::WebClaimsV1(_),
            ..
        }
    ));
}

#[tokio::test]
async fn a_json_runner_cannot_satisfy_a_web_claims_profile() {
    let mut setup = Setup::new();
    setup.draft.output_kind = OutputKind::WebClaimsV1;
    setup.draft.output_schema = None;
    setup.web = ScriptRunner::new(vec![json_ok(), json_ok()]);
    let harness = Harness::build(setup);
    let task_id = harness.admit_task_id();
    assert_eq!(
        code_of(&settle(&harness, &task_id).await),
        FailureCode::CompletionUnmet
    );
}

#[tokio::test]
async fn an_unsettled_write_call_is_not_retried() {
    let json = ScriptRunner::with_hook(vec![Err(AttemptError::Transport("boom".into()))], |env| {
        env.writer
                .transact(|c| {
                    c.execute(
                        "INSERT INTO worker_tool_calls(task_id, operation_key, attempt_ordinal, tool_key,
                            effect, dispatch_state, created_at_ms, updated_at_ms)
                         VALUES(?1, '1:1:web_search:abc', 1, 'web_search', 'write', 'dispatched', 1, 1)",
                        params![env.task_id],
                    )
                    .map(|_| ())
                    .map_err(|e| e.to_string())
                })
                .unwrap();
    });
    let mut setup = Setup::new();
    setup.json = json.clone();
    let harness = Harness::build(setup);
    let task_id = harness.admit_task_id();
    let outcome = settle(&harness, &task_id).await;
    assert_eq!(code_of(&outcome), FailureCode::OutcomeUnknown);
    assert_eq!(json.calls(), 1);
}

#[tokio::test]
async fn an_async_terminal_enqueues_exactly_one_outbox_report() {
    let harness = Harness::build(Setup::new());
    let task_id = harness.admit_task_id();
    assert!(matches!(
        harness.exec.wait_terminal(&task_id, Duration::ZERO).await,
        WorkerOutcome::Pending { .. }
    ));
    assert_eq!(harness.task(&task_id).delivery, "async_queued");
    // JsonRunner with no scripted reply fails; script a success instead.
    harness.exec_sql("UPDATE worker_tasks SET state='running' WHERE state='accepted'");
    let finalized = harness
        .writer
        .transact(|c| {
            finalize(
                c,
                &task_id,
                &Terminal::Succeeded(WorkerOutput::JsonV1(serde_json::json!({"a": 1}))),
                now_ms(),
            )
        })
        .unwrap();
    assert!(finalized.changed && finalized.reported);
    let again = harness
        .writer
        .transact(|c| {
            finalize(
                c,
                &task_id,
                &Terminal::Failed(FailureCode::Interrupted),
                now_ms(),
            )
        })
        .unwrap();
    assert!(!again.changed && !again.reported);
    assert_eq!(
        harness.count("SELECT count(*) FROM steward_reports WHERE task_id LIKE 'wtask_%'"),
        1
    );
    let task = harness.task(&task_id);
    assert_eq!(
        (task.state.as_str(), task.delivery.as_str()),
        ("succeeded", "async_delivered")
    );
}

#[tokio::test]
async fn processing_an_async_task_reports_once_and_calls_the_hook() {
    let json = ScriptRunner::new(vec![Err(AttemptError::Terminal(
        FailureCode::NoSafeSources,
    ))]);
    let mut setup = Setup::new();
    setup.json = json;
    let harness = Harness::build(setup);
    let task_id = harness.admit_task_id();
    let _ = harness.exec.wait_terminal(&task_id, Duration::ZERO).await;
    harness.run_next().await.unwrap();
    let digest: String = harness
        .writer
        .read_serialized(|c| {
            c.query_row("SELECT digest FROM steward_reports", [], |row| row.get(0))
                .map_err(|e| e.to_string())
        })
        .unwrap();
    assert_eq!(
        digest,
        "安全に確認できる情報源が見つからず、確認できませんでした。"
    );
    assert_eq!(harness.count("SELECT count(*) FROM steward_reports"), 1);
    assert_eq!(
        *harness.reports.lock().unwrap(),
        [crate::PRIMARY_CONVERSATION_ID]
    );
}

#[tokio::test]
async fn a_task_finishing_inside_the_wait_is_delivered_synchronously() {
    let mut setup = Setup::new();
    setup.json = ScriptRunner::new(vec![json_ok()]);
    let harness = Harness::build(setup);
    let task_id = harness.admit_task_id();
    let (processed, outcome) = tokio::join!(
        harness.run_next(),
        harness.exec.wait_terminal(&task_id, Duration::from_secs(5))
    );
    processed.unwrap();
    assert!(
        matches!(outcome, WorkerOutcome::Succeeded { .. }),
        "{outcome:?}"
    );
    assert_eq!(harness.task(&task_id).delivery, "sync_delivered");
    assert_eq!(harness.count("SELECT count(*) FROM steward_reports"), 0);
    assert!(harness.reports.lock().unwrap().is_empty());
}

#[tokio::test]
async fn a_running_task_past_the_wait_becomes_async_and_stays_pending() {
    let harness = Harness::build(Setup::new());
    let task_id = harness.admit_task_id();
    let outcome = harness
        .exec
        .wait_terminal(&task_id, Duration::from_millis(30))
        .await;
    assert_eq!(
        outcome,
        WorkerOutcome::Pending {
            task_id: task_id.clone()
        }
    );
    assert_eq!(harness.task(&task_id).delivery, "async_queued");
    // A replayed admission of the same delegation also stays pending, not a second result.
    assert_eq!(harness.admit(), WorkerOutcome::Pending { task_id });
}

#[tokio::test]
async fn cancelling_the_input_before_the_run_never_reaches_the_outbox() {
    let harness = Harness::build(Setup::new());
    let task_id = harness.admit_task_id();
    let _ = harness.exec.wait_terminal(&task_id, Duration::ZERO).await;
    let cancelled = harness
        .writer
        .transact(|c| cancel_for_input(c, crate::PRIMARY_CONVERSATION_ID, MESSAGE_ID))
        .unwrap();
    assert_eq!(cancelled, [task_id.clone()]);
    assert!(harness.claim().is_none(), "the queue job is cancelled");
    let task = harness.task(&task_id);
    assert_eq!(
        (
            task.state.as_str(),
            task.delivery.as_str(),
            task.failure_code.as_deref()
        ),
        ("cancelled", "suppressed", Some("cancelled"))
    );
    assert_eq!(harness.count("SELECT count(*) FROM steward_reports"), 0);
    assert_eq!(
        code_of(&harness.exec.wait_terminal(&task_id, Duration::ZERO).await),
        FailureCode::Cancelled
    );
}

#[tokio::test]
async fn cancelling_a_running_task_stops_the_attempt_and_stays_silent() {
    let mut setup = Setup::new();
    setup.json = ScriptRunner::blocking();
    let harness = Harness::build(setup);
    let task_id = harness.admit_task_id();
    let _ = harness.exec.wait_terminal(&task_id, Duration::ZERO).await;
    let (processed, ()) = tokio::join!(harness.run_next(), async {
        tokio::time::sleep(Duration::from_millis(80)).await;
        let cancelled = harness
            .writer
            .transact(|c| cancel_for_input(c, crate::PRIMARY_CONVERSATION_ID, MESSAGE_ID))
            .unwrap();
        assert_eq!(cancelled.len(), 1);
    });
    processed.unwrap();
    let task = harness.task(&task_id);
    assert_eq!(
        (task.state.as_str(), task.delivery.as_str()),
        ("cancelled", "suppressed")
    );
    assert_eq!(harness.count("SELECT count(*) FROM steward_reports"), 0);
    assert_eq!(
        harness.count("SELECT count(*) FROM worker_attempts WHERE status='cancelled'"),
        1
    );
}

#[tokio::test]
async fn a_task_past_its_deadline_fails_without_calling_the_model() {
    let harness = Harness::build(Setup::new());
    let task_id = harness.admit_task_id();
    harness.exec_sql("UPDATE worker_tasks SET deadline_at_ms = 1");
    let outcome = settle(&harness, &task_id).await;
    assert_eq!(code_of(&outcome), FailureCode::DeadlineExceeded);
    assert_eq!(harness.count("SELECT count(*) FROM worker_attempts"), 0);
}

#[test]
fn digests_are_host_templates() {
    let output = claims_output("https://www.example.com/a/b?x=1");
    let text = digest_for(&Terminal::Succeeded(output));
    assert_eq!(
        text,
        "先ほどの調査結果です（Web由来の情報）。桜は例年三月下旬に開花する（example.com）"
    );
    assert_eq!(
        digest_for(&Terminal::Failed(FailureCode::NoSafeSources)),
        "安全に確認できる情報源が見つからず、確認できませんでした。"
    );
    let WorkerOutput::WebClaimsV1(mut claims) = claims_output("https://a.example/") else {
        unreachable!()
    };
    claims.claims = (0..5).map(|_| claims.claims[0].clone()).collect();
    let many = digest_for(&Terminal::Succeeded(WorkerOutput::WebClaimsV1(claims)));
    // The Web-origin marker plus one host mark per claim, capped at three claims.
    assert_eq!(many.matches('（').count(), 1 + 3);
}

#[test]
fn compose_system_pins_context_then_skills_in_order() {
    let harness = Harness::build(Setup::new());
    let mut revision = harness
        .writer
        .read_serialized(|c| load_revision(c, &harness.revision_id))
        .unwrap();
    revision.skills = vec![
        LoadedSkill {
            name: "first".into(),
            body: "BODY-1".into(),
        },
        LoadedSkill {
            name: "second".into(),
            body: "BODY-2".into(),
        },
    ];
    let system = compose_system(&revision);
    assert!(system.starts_with("You are a narrow worker."));
    let first = system.find("## Skill: first\nBODY-1").unwrap();
    let second = system.find("## Skill: second\nBODY-2").unwrap();
    assert!(first < second);
    let _ = JsonRunner;
}
