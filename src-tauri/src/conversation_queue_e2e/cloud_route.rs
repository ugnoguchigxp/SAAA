//! Conversation on a direct cloud Chat Completions service with LARM unreachable.
use super::*;
use crate::providers::service_registry::{
    migrate_legacy, AdapterKind, BindingReview, Capability, Purpose, ServiceConnection,
    ServiceResource,
};

pub(super) async fn run_with_server(base: &str, fixture: &Arc<Fixture>) -> Result<Value, String> {
    let mut connection = Connection::open_in_memory().map_err(crate::database_error)?;
    persistence::schema::initialize_database(&connection).map_err(crate::database_error)?;
    let loaded = persistence::service_registry_store::load_registry(&connection)?;
    let mut snapshot = migrate_legacy(
        &persistence::load_model_providers(&connection)?,
        &persistence::load_routing_settings(&connection)?,
    )?;
    // Nothing listens on this LARM address: any LARM request would fail the job.
    snapshot.connections[0].endpoint = "http://127.0.0.1:1".into();
    snapshot.connections.push(ServiceConnection {
        connection_id: "conn:cloud-llm".into(),
        label: "Cloud".into(),
        adapter_kind: AdapterKind::ChatCompletions,
        endpoint: format!("{base}/llm/v1"),
        location: "cloud".into(),
        authentication: "none".into(),
        credential_ref: None,
        enabled: true,
    });
    snapshot.resources.push(ServiceResource {
        resource_id: "res:cloud-llm".into(),
        connection_id: "conn:cloud-llm".into(),
        capability: Capability::TextGeneration,
        model: "fixture-cloud".into(),
        detail: None,
        enabled: true,
    });
    let binding = snapshot
        .bindings
        .iter_mut()
        .find(|binding| binding.purpose == Purpose::ConversationRespond)
        .ok_or("conversation binding missing")?;
    binding.primary_resource_id = Some("res:cloud-llm".into());
    binding.stored_primary_resource_id = None;
    binding.review = BindingReview::Ready;
    persistence::service_registry_store::save_registry(
        &mut connection,
        &snapshot,
        loaded.revision,
    )?;
    let mut providers: Value = connection
        .query_row(
            "SELECT value_json FROM settings_documents WHERE namespace='providers.model' AND key='default'",
            [],
            |row| row.get::<_, String>(0),
        )
        .map_err(crate::database_error)
        .and_then(|value| serde_json::from_str(&value).map_err(|error| error.to_string()))?;
    providers["harness"]["address"] = json!("http://127.0.0.1:1");
    connection
        .execute(
            "UPDATE settings_documents SET value_json=?1 WHERE namespace='providers.model' AND key='default'",
            [providers.to_string()],
        )
        .map_err(crate::database_error)?;
    let state = crate::test_support::app_state(connection);
    let app = tauri::test::mock_builder()
        .manage(state)
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .map_err(|error| error.to_string())?;
    let state = app.state::<AppState>();
    let key = "cloud-route-research";
    conversation_check::queue_runtime::enqueue_text(
        &state,
        key.into(),
        "今日の事実を調べて".into(),
    )?;
    conversation_check::spawn_queue_workers(app.handle().clone());
    state.conversation_queue_wake.notify_waiters();
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(30);
    let answer = loop {
        let saved: Option<String> = state.sqlite_readers.read(|connection| {
            connection
                .query_row(
                    "SELECT content FROM conversation_messages WHERE id=?1",
                    [format!("reply_{key}")],
                    |row| row.get(0),
                )
                .optional()
                .map_err(crate::database_error)
        })?;
        if let Some(saved) = saved {
            break saved;
        }
        if tokio::time::Instant::now() > deadline {
            return Err(format!(
                "cloud conversation did not answer; calls={:?}",
                fixture.calls.lock().ok()
            ));
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    };
    let calls = fixture.calls.lock().map_err(|_| "calls lock")?.clone();
    let larm_requests = calls
        .iter()
        .filter(|call| call.contains("agent-connections") || call.contains("agent-profiles"))
        .count();
    let llm_calls = *fixture.llm_calls.lock().map_err(|_| "LLM count lock")?;
    Ok(json!({"answer":answer,"larmRequests":larm_requests,"llmCalls":llm_calls}))
}
