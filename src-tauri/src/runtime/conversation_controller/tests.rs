#![cfg(test)]
use super::*;
#[test]
fn cancelled_run_cannot_accept_output() {
    let cancellation = RunCancellation::default();
    cancellation.cancel();
    let mut accepted = false;
    assert!(cancellation
        .with_active(|| {
            accepted = true;
            Ok(())
        })
        .is_err());
    assert!(!accepted);
}
#[test]
fn projection_removes_only_current_user_message() {
    let input: StartTurnInput = serde_json::from_value(serde_json::json!({
        "runId":"run_test","conversationId":"conversation_test","content":"比較して",
        "workspacePath":null,"retryInputMessageId":null,"sourceId":null,"inputOrigin":"voice","presentationMode":"visual"
    })).unwrap();
    let history: Vec<ConversationMessage> = ["比較して", "追加条件です", "比較して"]
        .into_iter()
        .enumerate()
        .map(|(i, content)| ConversationMessage {
            parts: None,
            id: format!("message_{i}"),
            conversation_id: input.conversation_id.clone(),
            role: "user".into(),
            content: content.into(),
            created_at: "now".into(),
        })
        .collect();
    let projected = project(&input, &history).unwrap();
    assert_eq!(projected.context.messages.len(), 2);
    assert_eq!(projected.context.messages[0].content, "比較して");
    assert_eq!(projected.request, "比較して");
}

#[test]
fn wd_10_reasoning_request_carries_world_as_typed_evidence() {
    let input: StartTurnInput = serde_json::from_value(serde_json::json!({
        "runId":"run_world","conversationId":"conversation_world","content":"いま何を進めていますか",
        "workspacePath":null,"retryInputMessageId":null,"sourceId":null,"inputOrigin":"voice","presentationMode":"visual"
    }))
    .unwrap();
    let mut request = project(&input, &[]).unwrap();
    let world = crate::runtime::context::source::Candidate::untrusted(
        "world".into(),
        crate::runtime::context::world::source::WORLD_KIND,
        vec!["project:one".into()],
        crate::runtime::context::source::Requirement::May,
        "frame-1".into(),
        4,
        100,
        concat!("[WORLD_MODEL — untrusted data; instructionAuthority=none]\n", r#"{"schema_version":2,"run_id":"run_world","scope":{"focus_scope_key":"project:one","allowed_scope_keys":["project:one"],"digest":"scope"},"sources":[],"project_scope":"project:one","captured_at_ms":1000,"expires_at_ms":2000,"graph":null,"runtime":[],"runtime_focus":[],"notices":[],"truncated":false}"#, "\n[END_WORLD_MODEL]").into(),
    );
    fit_context(&mut request, &[world]).unwrap();
    assert!(request
        .context
        .evidence
        .iter()
        .any(|evidence| evidence.source.starts_with("world-model:frame-1@4")));
}

#[tokio::test]
async fn reasoning_roundtrip_commits_only_valid_answer() {
    use crate::providers::reasoning_mcp::{tests::fixture, Client};
    for mode in ["good", "stale"] {
        let connection = rusqlite::Connection::open_in_memory().unwrap();
        crate::initialize_database(&connection).unwrap();
        let state = crate::test_support::app_state(connection);
        let input:StartTurnInput=serde_json::from_value(serde_json::json!({
            "runId":"run_reasoning_test","conversationId":crate::PRIMARY_CONVERSATION_ID,"content":"比較してください",
            "inputOrigin":"voice","presentationMode":"visual"
        })).unwrap();
        crate::runtime::turns::prepare_runtime_run(&state, &input).unwrap();
        let server = fixture(mode).await;
        let client = Client::new(&server.url, "fixture-token-long-enough".into()).unwrap();
        let channel = tauri::ipc::Channel::<RuntimeEvent>::new(|_| Ok(()));
        let result = execute(
            &state,
            &input,
            &[],
            &channel,
            Arc::default(),
            &client,
            ContextManifest {
                selected: &[],
                omitted: &[],
                health: "green",
                world: None,
            },
        )
        .await;
        assert_eq!(result.is_ok(), mode == "good", "{result:?}");
        let count = state
            .sqlite_readers
            .read(|c| {
                c.query_row(
                    "SELECT COUNT(*) FROM conversation_messages WHERE role='assistant'",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .map_err(crate::database_error)
            })
            .unwrap();
        assert_eq!(count, if mode == "good" { 1 } else { 0 });
    }
}

#[tokio::test]
async fn reasoning_ack_is_skipped_for_fast_result_and_polled_with_slow_result() {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Duration;
    let ack = AtomicBool::new(false);
    assert_eq!(
        with_ack(
            async { 42 },
            async {
                ack.store(true, Ordering::SeqCst);
            },
            Duration::from_millis(5)
        )
        .await,
        42
    );
    assert!(!ack.load(Ordering::SeqCst));
    let ready = tokio::sync::Notify::new();
    let result = with_ack(
        async {
            ready.notified().await;
            42
        },
        async {
            ack.store(true, Ordering::SeqCst);
            ready.notify_one();
        },
        Duration::from_millis(5),
    );
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(1), result)
            .await
            .unwrap(),
        42
    );
    assert!(ack.load(Ordering::SeqCst));
}

#[test]
fn projection_trims_to_the_actual_provider_budget_without_truncating_the_request() {
    let input: StartTurnInput = serde_json::from_value(serde_json::json!({
        "runId":"run_test","conversationId":"conversation_test","content":"現在の条件で比較して",
        "inputOrigin":"voice","presentationMode":"visual"
    }))
    .unwrap();
    let history = (0..20)
        .map(|i| ConversationMessage {
            parts: None,
            id: format!("message_{i}"),
            conversation_id: input.conversation_id.clone(),
            role: "assistant".into(),
            content: "日本語の会話履歴。".repeat(200),
            created_at: "now".into(),
        })
        .collect::<Vec<_>>();
    let mut request = project(&input, &history).unwrap();
    fit_context(&mut request, &[]).unwrap();
    assert!(request.model_input_fits());
    assert!(request.context.truncated);
    assert!(request.context.messages.len() < history.len());
    assert_eq!(request.request, input.content);
    let mut oversized = input;
    oversized.content = "あ".repeat(10_000);
    let mut oversized_request = project(&oversized, &[]).unwrap();
    assert!(fit_context(&mut oversized_request, &[]).is_err());
}

#[test]
fn fitting_reasoning_context_keeps_required_state() {
    let input: StartTurnInput = serde_json::from_value(serde_json::json!({
        "runId":"run_test","conversationId":"conversation_test","content":"続けて",
        "inputOrigin":"voice","presentationMode":"visual"
    }))
    .unwrap();
    let required = crate::runtime::context::source::Candidate::untrusted(
        "state".into(),
        "personal-state",
        vec![],
        crate::runtime::context::source::Requirement::Must,
        "state".into(),
        1,
        1,
        r#"{"status":"Active","value":"外部送信しない"}"#.into(),
    );
    let history = (0..40)
        .map(|index| ConversationMessage {
            parts: None,
            id: format!("message_{index}"),
            conversation_id: input.conversation_id.clone(),
            role: "assistant".into(),
            content: if index == 0 {
                format!("state: {}", required.content)
            } else {
                "任意履歴。".repeat(500)
            },
            created_at: "now".into(),
        })
        .collect::<Vec<_>>();
    let mut request = project(&input, &history).unwrap();
    fit_context(&mut request, std::slice::from_ref(&required)).unwrap();
    assert!(request
        .context
        .evidence
        .iter()
        .any(|evidence| evidence.content == required.content));
}
