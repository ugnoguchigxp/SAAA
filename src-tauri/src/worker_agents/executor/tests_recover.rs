use super::recover;
use super::test_fakes::*;
use crate::worker_agents::contracts::*;
use crate::worker_agents::loader::now_ms;

fn run_recovery(harness: &Harness) -> Vec<String> {
    harness.writer.transact(|c| recover(c, now_ms())).unwrap()
}

#[test]
fn a_read_only_task_is_requeued_once_then_interrupted() {
    let harness = Harness::build(Setup::new());
    let task_id = harness.admit_task_id();
    harness.exec_sql("UPDATE worker_tasks SET state='running'");

    assert!(run_recovery(&harness).is_empty());
    let task = harness.task(&task_id);
    assert_eq!(
        (task.state.as_str(), task.delivery.as_str(), task.restarts),
        ("accepted", "async_queued", 1)
    );
    // Safe to call twice: nothing changes while the task waits for its (single) job.
    assert!(run_recovery(&harness).is_empty());
    assert_eq!(harness.task(&task_id).restarts, 1);
    assert_eq!(
        harness
            .count("SELECT count(*) FROM task_queue_jobs WHERE lane='worker' AND state='queued'"),
        1
    );

    // The restarted run is interrupted again: the restart budget is spent.
    harness.exec_sql("UPDATE worker_tasks SET state='running'");
    let flush = run_recovery(&harness);
    assert_eq!(flush, [crate::PRIMARY_CONVERSATION_ID]);
    let task = harness.task(&task_id);
    assert_eq!(
        (task.state.as_str(), task.failure_code.as_deref()),
        ("failed", Some("interrupted"))
    );
    assert_eq!(harness.count("SELECT count(*) FROM steward_reports"), 1);
    assert!(run_recovery(&harness).is_empty());
    assert_eq!(harness.count("SELECT count(*) FROM steward_reports"), 1);
}

#[test]
fn an_unsettled_write_call_is_never_replayed_after_a_restart() {
    let harness = Harness::build(Setup::new());
    let task_id = harness.admit_task_id();
    harness.exec_sql(&format!(
        "UPDATE worker_tasks SET state='running' WHERE id='{task_id}';
         INSERT INTO worker_tool_calls(task_id, operation_key, attempt_ordinal, tool_key, effect,
            dispatch_state, created_at_ms, updated_at_ms)
         VALUES('{task_id}', '1:1:web_search:abc', 1, 'web_search', 'write', 'dispatched', 1, 1);
         INSERT INTO worker_attempts(task_id, ordinal, tier, route_fingerprint, status, started_at_ms)
         VALUES('{task_id}', 1, 'local', 'l0', 'running', 1);"
    ));
    run_recovery(&harness);
    let task = harness.task(&task_id);
    assert_eq!(
        (
            task.state.as_str(),
            task.failure_code.as_deref(),
            task.restarts
        ),
        ("failed", Some("interrupted"), 0)
    );
    assert_eq!(
        harness.count("SELECT count(*) FROM worker_attempts WHERE status='interrupted'"),
        1
    );
    assert_eq!(harness.count("SELECT count(*) FROM steward_reports"), 1);
}

#[test]
fn an_unsettled_read_call_still_allows_one_restart() {
    let harness = Harness::build(Setup::new());
    let task_id = harness.admit_task_id();
    harness.exec_sql(&format!(
        "UPDATE worker_tasks SET state='verifying' WHERE id='{task_id}';
         INSERT INTO worker_tool_calls(task_id, operation_key, attempt_ordinal, tool_key, effect,
            dispatch_state, created_at_ms, updated_at_ms)
         VALUES('{task_id}', '1:1:web_search:abc', 1, 'web_search', 'read', 'dispatched', 1, 1);"
    ));
    run_recovery(&harness);
    assert_eq!(harness.task(&task_id).state, "accepted");
}

#[test]
fn recovery_gives_every_survivor_a_live_job_even_if_the_lane_was_interrupted() {
    let harness = Harness::build(Setup::new());
    let task_id = harness.admit_task_id();
    harness
        .writer
        .transact(|c| crate::task_queue::recover(c, &[], &[WORKER_LANE]))
        .unwrap();
    assert!(harness.claim().is_none());
    run_recovery(&harness);
    let job = harness.claim().expect("a fresh job was queued");
    assert_eq!(job.generation, 1);
    assert_eq!(job.key, task_id);
    assert_eq!(harness.task(&task_id).delivery, "async_queued");
}

#[tokio::test]
async fn a_recovered_task_runs_to_completion_through_the_executor() {
    let mut setup = Setup::new();
    setup.json = ScriptRunner::new(vec![json_ok()]);
    let harness = Harness::build(setup);
    let task_id = harness.admit_task_id();
    harness.exec_sql("UPDATE worker_tasks SET state='running'");
    harness
        .writer
        .transact(|c| crate::task_queue::recover(c, &[WORKER_LANE], &[]))
        .unwrap();
    assert_eq!(harness.exec.recover().unwrap(), 0);
    harness.run_next().await.unwrap();
    let task = harness.task(&task_id);
    assert_eq!(task.state, "succeeded");
    // Delivered asynchronously: the waiting conversation job did not survive the restart.
    assert_eq!(task.delivery, "async_delivered");
    assert_eq!(harness.count("SELECT count(*) FROM steward_reports"), 1);
    assert_eq!(harness.reports.lock().unwrap().len(), 1);
}

#[test]
fn abandoning_a_failed_job_settles_its_task_as_interrupted_once() {
    let harness = Harness::build(Setup::new());
    let task_id = harness.admit_task_id();
    harness.exec_sql("UPDATE worker_tasks SET state='running'");
    let payload = format!("{{\"taskId\":\"{task_id}\"}}");
    harness
        .writer
        .transact(|c| super::abandon_job(c, &payload))
        .unwrap();
    let task = harness.task(&task_id);
    assert_eq!(
        (task.state.as_str(), task.failure_code.as_deref()),
        ("failed", Some("interrupted"))
    );
    // Repeating it, or a payload without a task id, changes nothing.
    harness
        .writer
        .transact(|c| super::abandon_job(c, &payload))
        .unwrap();
    harness
        .writer
        .transact(|c| super::abandon_job(c, "not json"))
        .unwrap();
    assert_eq!(harness.task(&task_id).state, "failed");
}
