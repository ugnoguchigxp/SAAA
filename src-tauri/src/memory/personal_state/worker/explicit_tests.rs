#![cfg(test)]
use super::*;
use crate::memory::personal_state::world::test_support::{ensure_scope, memory_db};
use std::sync::atomic::{AtomicUsize, Ordering};
struct LocalFixture {
    calls: AtomicUsize,
    invalid: bool,
}
#[async_trait::async_trait]
impl Extractor for LocalFixture {
    async fn extract(&self, _: Value, _: Arc<RunCancellation>) -> Result<String, String> {
        Ok(json!({"candidates":[],"no_change":true}).to_string())
    }
    async fn extract_world(
        &self,
        input: Value,
        _: Arc<RunCancellation>,
    ) -> Result<Option<String>, String> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if self.invalid {
            return Ok(Some("not-json".into()));
        }
        let text = input["source"]["text"].as_str().unwrap();
        Ok(Some(json!({"candidates":[{"kind":"world_entity","payload":{"schema_version":2,"type":"entity","entity_id":"cache","entity_kind":"concept","name":"キャッシュ","aliases":[],"objective_assertion_id":null},"quote":text,"quote_start":0,"quote_end":text.len(),"epistemic":"user_reported","replaces":null}],"no_change":false}).to_string()))
    }
    fn provenance(&self) -> Provenance {
        UnavailableExtractor.provenance()
    }
}
fn fixture() -> (SqliteWriter, StartTurnInput) {
    let c = memory_db();
    let text = "Worldに登録して：キャッシュを変更した";
    c.execute(
        "INSERT INTO conversation_messages VALUES('older',?1,'user','old unrelated input','1')",
        [crate::PRIMARY_CONVERSATION_ID],
    )
    .unwrap();
    c.execute(
        "INSERT INTO conversation_messages VALUES('explicit',?1,'user',?2,?3)",
        params![
            crate::PRIMARY_CONVERSATION_ID,
            text,
            crate::memory::personal_state::now().to_string()
        ],
    )
    .unwrap();
    ensure_scope(&c, "project:explicit");
    c.execute(
        "INSERT INTO personal_source_scope_refs VALUES('explicit',1,'project:explicit')",
        [],
    )
    .unwrap();
    c.execute(
        "UPDATE personal_jobs SET scope_key='project:explicit' WHERE source_sequence=2",
        [],
    )
    .unwrap();
    c.execute("INSERT INTO runtime_runs(id,conversation_id,route_kind,status,started_at,input_message_id) VALUES('explicit-run',?1,'conversation.respond','running','1','explicit')",[crate::PRIMARY_CONVERSATION_ID]).unwrap();
    let input = StartTurnInput {
        run_id: "explicit-run".into(),
        conversation_id: crate::PRIMARY_CONVERSATION_ID.into(),
        content: text.into(),
        workspace_path: None,
        retry_input_message_id: None,
        source_id: None,
        scope_refs: vec![],
        input_origin: "text".into(),
        presentation_mode: "visual".into(),
    };
    (SqliteWriter::from_connection(c), input)
}
#[tokio::test]
async fn world_explicit_registers_current_source_during_foreground_and_replays_without_duplicates()
{
    let (writer, input) = fixture();
    writer
        .read_serialized(|c| {
            assert!(jobs::claim(c, crate::memory::personal_state::now(), true)?.is_none());
            Ok(())
        })
        .unwrap();
    let local = LocalFixture {
        calls: AtomicUsize::new(0),
        invalid: false,
    };
    let result = extract_now(
        &writer,
        &local,
        &input,
        Arc::new(RunCancellation::default()),
    )
    .await
    .unwrap();
    assert_eq!(result["status"], "registered");
    assert_eq!(result["worldAssertions"], 1);
    let replay = extract_now(
        &writer,
        &local,
        &input,
        Arc::new(RunCancellation::default()),
    )
    .await
    .unwrap();
    assert_eq!(replay["status"], "already_processed");
    assert_eq!(local.calls.load(Ordering::SeqCst), 1);
    writer
        .read_serialized(|c| {
            assert_eq!(
                c.query_row(
                    "SELECT status FROM personal_jobs WHERE source_sequence=1",
                    [],
                    |r| r.get::<_, String>(0)
                )
                .unwrap(),
                "queued"
            );
            Ok(())
        })
        .unwrap();
}
#[test]
fn world_explicit_requires_unquoted_directive_current_turn_scope_and_available_source() {
    assert!(requested("これを覚えて。キャッシュの設定を変更した"));
    for text in [
        "普通の会話",
        "Worldに登録しないで",
        "『Worldに登録して』という例",
        "> Worldに登録して",
        "説明\nWorldに登録して",
        "```\nWorldに登録して\n```",
    ] {
        assert!(!requested(text), "{text}");
    }
    let (writer, mut input) = fixture();
    input.content = "これを覚えて。偽の入力".into();
    assert_eq!(
        writer.transact(|c| claim(c, &input)).unwrap_err(),
        "world-current-turn-unavailable"
    );
    input.content = "Worldに登録して：キャッシュを変更した".into();
    writer
        .transact(|c| {
            c.execute(
                "DELETE FROM personal_source_scope_refs WHERE source_id='explicit'",
                [],
            )
            .unwrap();
            c.execute(
                "UPDATE personal_jobs SET scope_key=NULL WHERE source_sequence=2",
                [],
            )
            .unwrap();
            Ok(())
        })
        .unwrap();
    assert_eq!(
        writer.transact(|c| claim(c, &input)).unwrap_err(),
        "world-scope-unresolved"
    );
    writer
        .transact(|c| {
            c.execute("DELETE FROM conversation_messages WHERE id='explicit'", [])
                .unwrap();
            Ok(())
        })
        .unwrap();
    assert_eq!(
        writer.transact(|c| claim(c, &input)).unwrap_err(),
        "world-current-turn-unavailable"
    );
}
#[tokio::test]
async fn world_explicit_failure_preserves_world_checkpoint_and_does_not_report_registration() {
    let (writer, input) = fixture();
    let local = LocalFixture {
        calls: AtomicUsize::new(0),
        invalid: true,
    };
    assert!(extract_now(
        &writer,
        &local,
        &input,
        Arc::new(RunCancellation::default())
    )
    .await
    .is_err());
    writer
        .read_serialized(|c| {
            let (status, stage): (String, String) = c
                .query_row(
                    "SELECT status,stage FROM personal_jobs WHERE source_sequence=2",
                    [],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .unwrap();
            assert_eq!((status.as_str(), stage.as_str()), ("queued", "world"));
            assert!(store::load(c)?
                .assertions
                .values()
                .all(|a| !a.kind.is_world()));
            Ok(())
        })
        .unwrap();
}
#[tokio::test]
async fn world_explicit_tool_rejects_argument_payloads_and_missing_host_without_io() {
    let (_, input) = fixture();
    let cancel = RunCancellation::default();
    let value: Value = serde_json::from_str(
        &execute(
            None,
            &input,
            "{\"source_id\":\"other\"}",
            std::time::Duration::from_secs(1),
            &cancel,
        )
        .await,
    )
    .unwrap();
    assert_eq!(value["reason"], "invalid-arguments");
    let value: Value = serde_json::from_str(
        &execute(
            None,
            &input,
            "{}",
            std::time::Duration::from_secs(1),
            &cancel,
        )
        .await,
    )
    .unwrap();
    assert_eq!(value["reason"], "current-turn-unavailable");
}

#[tokio::test]
async fn world_explicit_is_offered_only_for_direct_requests_and_routes_through_agent_dispatch() {
    let (writer, mut input) = fixture();
    let mut state = crate::test_support::app_state(memory_db());
    state.sqlite_writer = Arc::new(writer);
    state.sqlite_readers =
        crate::persistence::SqliteReaders::serialized(state.sqlite_writer.clone());
    let persistence = crate::ProviderOutputPersistence {
        state: &state,
        session_id: "explicit-session",
        world: None,
    };
    let offered =
        crate::providers::stream::available_agent_tools(Some(persistence), &input, 0, 0, 0);
    assert!(offered
        .definitions
        .iter()
        .any(|d| d["function"]["name"] == TOOL));
    let exhausted =
        crate::providers::stream::available_agent_tools(Some(persistence), &input, 12, 0, 0);
    assert!(exhausted
        .definitions
        .iter()
        .all(|d| d["function"]["name"] != TOOL));
    let call = crate::runtime::agent_tools::AgentToolCall {
        id: "explicit-call".into(),
        name: TOOL.into(),
        arguments: "{\"source_id\":\"other\"}".into(),
    };
    let raw = crate::providers::stream::execute_agent_tool(
        Some(persistence),
        &input,
        &call,
        std::time::Duration::from_secs(1),
        &offered.generated,
        &RunCancellation::default(),
        None,
    )
    .await;
    let result: Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(result["reason"], "invalid-arguments");
    input.content = "普通の会話".into();
    let offered =
        crate::providers::stream::available_agent_tools(Some(persistence), &input, 0, 0, 0);
    assert!(offered
        .definitions
        .iter()
        .all(|d| d["function"]["name"] != TOOL));
}

struct NoWorld;
#[async_trait::async_trait]
impl Extractor for NoWorld {
    async fn extract(&self, _: Value, _: Arc<RunCancellation>) -> Result<String, String> {
        Ok(json!({"candidates":[],"no_change":true}).to_string())
    }
    fn provenance(&self) -> Provenance {
        UnavailableExtractor.provenance()
    }
}
#[tokio::test]
async fn world_explicit_no_change_and_busy_never_claim_registration() {
    let (writer, input) = fixture();
    let result = extract_now(
        &writer,
        &NoWorld,
        &input,
        Arc::new(RunCancellation::default()),
    )
    .await
    .unwrap();
    assert_eq!(result["status"], "no_world_change");
    let (writer, input) = fixture();
    writer
        .transact(|c| {
            c.execute(
                "UPDATE personal_jobs SET status='running' WHERE source_sequence=1",
                [],
            )
            .unwrap();
            Ok(())
        })
        .unwrap();
    let local = LocalFixture {
        calls: AtomicUsize::new(0),
        invalid: false,
    };
    assert_eq!(
        extract_now(
            &writer,
            &local,
            &input,
            Arc::new(RunCancellation::default())
        )
        .await
        .unwrap_err(),
        "world-registration-busy"
    );
    assert_eq!(local.calls.load(Ordering::SeqCst), 0);
}
#[tokio::test]
async fn world_explicit_cancel_keeps_source_retryable_without_world_commit() {
    let (writer, input) = fixture();
    let cancel = Arc::new(RunCancellation::default());
    cancel.cancel();
    assert!(extract_now(&writer, &NoWorld, &input, cancel)
        .await
        .is_err());
    writer
        .read_serialized(|c| {
            let (status, attempts, aborts): (String, u64, u64) = c
                .query_row(
                    "SELECT status,attempts,abort_count FROM personal_jobs WHERE source_sequence=2",
                    [],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                )
                .unwrap();
            assert_eq!((status.as_str(), attempts, aborts), ("queued", 0, 1));
            assert!(store::load(c)?
                .assertions
                .values()
                .all(|a| !a.kind.is_world()));
            Ok(())
        })
        .unwrap();
}

#[tokio::test]
async fn world_explicit_setup_uses_absolute_deadline_and_parent_or_memory_interrupt() {
    let parent = RunCancellation::default();
    let cancel = RunCancellation::default();
    let expired = tokio::time::Instant::now() - std::time::Duration::from_millis(1);
    let result = within_budget(
        std::future::pending::<Result<(), String>>(),
        expired,
        &parent,
        &cancel,
    )
    .await;
    assert_eq!(result.unwrap_err(), "personal-extraction-timeout");
    assert!(cancel.is_cancelled());
    let parent = RunCancellation::default();
    parent.cancel();
    let cancel = RunCancellation::default();
    let later = tokio::time::Instant::now() + std::time::Duration::from_secs(60);
    let result = within_budget(
        std::future::pending::<Result<(), String>>(),
        later,
        &parent,
        &cancel,
    )
    .await;
    assert_eq!(result.unwrap_err(), "personal-foreground-abort");
    assert!(cancel.is_cancelled());
    let parent = RunCancellation::default();
    let cancel = RunCancellation::default();
    cancel.cancel();
    let result = within_budget(
        std::future::pending::<Result<(), String>>(),
        later,
        &parent,
        &cancel,
    )
    .await;
    assert_eq!(result.unwrap_err(), "personal-foreground-abort");
}
