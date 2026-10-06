use super::test_fakes::*;
use super::LedgeredToolRunner;
use crate::worker_agents::contracts::*;
use crate::RunCancellation;
use std::time::Duration;

fn tool(key: &str, effect: ToolEffect) -> LoadedTool {
    LoadedTool {
        kind: ToolRefKind::Builtin,
        key: key.into(),
        catalog_revision_id: None,
        effect,
    }
}

fn rows(harness: &Harness) -> Vec<(String, String, String, Option<String>)> {
    harness
        .writer
        .read_serialized(|c| {
            let mut statement = c
                .prepare(
                    "SELECT operation_key, effect, dispatch_state, outcome FROM worker_tool_calls
                     ORDER BY operation_key",
                )
                .map_err(|e| e.to_string())?;
            let mapped = statement
                .query_map([], |row| {
                    Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
                })
                .map_err(|e| e.to_string())?;
            mapped
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| e.to_string())
        })
        .unwrap()
}

#[tokio::test]
async fn an_unlisted_tool_is_refused_without_calling_the_inner_runner() {
    let harness = Harness::build(Setup::new());
    let task_id = harness.admit_task_id();
    let inner = FakeTools::default();
    let tools = [tool("web_search", ToolEffect::Read)];
    let ledger = LedgeredToolRunner::new(&harness.writer, &task_id, 1, &tools, &inner);
    let reply = ledger
        .run(
            "shell",
            "{}",
            Duration::from_secs(1),
            &RunCancellation::default(),
        )
        .await;
    assert_eq!(reply, r#"{"error":{"code":"tool_not_allowed"}}"#);
    assert!(inner.calls.lock().unwrap().is_empty());
    assert!(rows(&harness).is_empty());
}

#[tokio::test]
async fn a_listed_call_goes_from_reserved_to_settled() {
    let harness = Harness::build(Setup::new());
    let task_id = harness.admit_task_id();
    let inner = FakeTools::default();
    let tools = [tool("web_search", ToolEffect::Read)];
    let ledger = LedgeredToolRunner::new(&harness.writer, &task_id, 2, &tools, &inner);
    let cancellation = RunCancellation::default();
    let reply = ledger
        .run(
            "web_search",
            r#"{"query":"桜"}"#,
            Duration::from_secs(1),
            &cancellation,
        )
        .await;
    assert_eq!(reply, r#"{"hits":[]}"#);
    ledger
        .run(
            "web_search",
            r#"{"query":"桜"}"#,
            Duration::from_secs(1),
            &cancellation,
        )
        .await;
    assert_eq!(ledger.steps(), 2);
    let rows = rows(&harness);
    assert_eq!(rows.len(), 2);
    for (key, effect, state, outcome) in &rows {
        assert!(key.starts_with("2:"), "operation key {key}");
        assert!(key.contains(":web_search:"));
        assert_eq!((effect.as_str(), state.as_str()), ("read", "settled"));
        assert_eq!(outcome.as_deref(), Some("ok bytes=11"));
    }
    assert_eq!(inner.calls.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn a_call_that_never_settles_stays_dispatched() {
    let harness = Harness::build(Setup::new());
    let task_id = harness.admit_task_id();
    let inner = FakeTools {
        hang: true,
        ..Default::default()
    };
    let tools = [tool("mutate", ToolEffect::Write)];
    let ledger = LedgeredToolRunner::new(&harness.writer, &task_id, 1, &tools, &inner);
    let timed_out = tokio::time::timeout(
        Duration::from_millis(50),
        ledger.run(
            "mutate",
            "{}",
            Duration::from_secs(1),
            &RunCancellation::default(),
        ),
    )
    .await;
    assert!(timed_out.is_err());
    let rows = rows(&harness);
    assert_eq!(rows.len(), 1);
    assert_eq!(
        (rows[0].1.as_str(), rows[0].2.as_str()),
        ("write", "dispatched")
    );
}

struct ToolCallingRunner;

#[async_trait::async_trait]
impl AttemptRunner for ToolCallingRunner {
    async fn run(&self, env: &AttemptEnv<'_>) -> Result<WorkerOutput, AttemptError> {
        let timeout = Duration::from_secs(1);
        let listed = env
            .tools
            .run("web_search", "{}", timeout, env.cancellation)
            .await;
        let unlisted = env.tools.run("rm", "{}", timeout, env.cancellation).await;
        assert_eq!(listed, r#"{"hits":[]}"#);
        assert_eq!(unlisted, r#"{"error":{"code":"tool_not_allowed"}}"#);
        Ok(WorkerOutput::JsonV1(serde_json::json!({"answer": 1})))
    }
}

#[tokio::test]
async fn the_executor_hands_attempts_a_ledgered_runner() {
    let mut setup = Setup::new();
    setup.json = std::sync::Arc::new(ToolCallingRunner);
    let tools = setup.tools.clone();
    let harness = Harness::build(setup);
    let task_id = harness.admit_task_id();
    harness.run_next().await.unwrap();
    assert!(matches!(
        harness.exec.wait_terminal(&task_id, Duration::ZERO).await,
        WorkerOutcome::Succeeded { .. }
    ));
    assert_eq!(*tools.calls.lock().unwrap(), ["web_search"]);
    assert_eq!(rows(&harness).len(), 1);
    assert_eq!(
        harness.count("SELECT steps_used FROM worker_attempts WHERE ordinal=1"),
        1
    );
}
