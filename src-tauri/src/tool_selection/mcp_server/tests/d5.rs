use super::*;
pub(crate) const ACCEPT_BOTH: &str = "application/json, text/event-stream";
pub(crate) struct D5 {
    pub(crate) server: ServerHandle,
    pub(crate) base: String,
    pub(crate) token: String,
    pub(crate) client: reqwest::Client,
    pub(crate) writer: Arc<SqliteWriter>,
    pub(crate) _directory: tempfile::TempDir,
}
pub(crate) fn token_value() -> String {
    use base64::Engine;
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode([0x42u8; 32])
}
pub(crate) fn tool_entry(index: usize, name: &str) -> CatalogEntry {
    CatalogEntry {
        tool_id: format!("tool-{index}"),
        backend_key: name.to_string(),
        title: name.to_string(),
        purpose: format!("Search notes and decision records for {name}."),
        operations: vec!["search".to_string(), "read".to_string()],
        objects: vec!["decision_record".to_string(), "document".to_string()],
        suitable: vec!["search notes".to_string()],
        unsuitable: vec!["unrelated chat".to_string()],
        required_inputs: vec!["q".to_string()],
        input_schema: json!({
            "type": "object",
            "properties": { "q": { "type": "string" } },
            "additionalProperties": false
        }),
        output_schema: None,
        effect: "read",
        usage_pages: vec![UsagePage {
            section: "usage",
            page: 0,
            text: format!("Use {name} for note lookup."),
        }],
        backend_binding: json!({ "capabilityId": format!("tool-{index}"), "revisionId": format!("tool-{index}-rev1") }),
    }
}
/// Builds the ledger, registers `count` tools, and returns the writer and service.
pub(crate) fn ledger(count: usize) -> (Arc<SqliteWriter>, Arc<ToolSelectionService>) {
    ledger_with_backend(count, Arc::new(FixtureBackend::new()))
}
pub(crate) fn ledger_with_backend(
    count: usize,
    backend: Arc<dyn ToolBackend>,
) -> (Arc<SqliteWriter>, Arc<ToolSelectionService>) {
    let connection = Connection::open_in_memory().expect("in-memory");
    crate::persistence::schema::initialize_database(&connection).expect("schema");
    let writer = Arc::new(SqliteWriter::from_connection(connection));
    let principal = service::ensure_principal(&writer).expect("principal");
    let embedding = Arc::new(HashEmbedding::new(384));
    let model_hash = embedding.model_hash().to_string();

    writer
        .write(|connection| {
            let transaction = connection
                .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
                .map_err(crate::database_error)?;
            for index in 0..count {
                let name = if index == 0 {
                    "search_notes".to_string()
                } else {
                    format!("tool_{index:04}")
                };
                let entry = tool_entry(index, &name);
                let revision_id = format!("tool-{index}-rev1");
                let vector: Vec<f32> = (0..384)
                    .map(|position| ((position + index) % 7) as f32 / 7.0)
                    .collect();
                catalog::register_revision(
                    &transaction,
                    &principal,
                    "llang",
                    &entry,
                    &revision_id,
                    now_ms(),
                )
                .map_err(|error| error.code.as_str().to_string())?;
                repository::upsert_embedding(&transaction, &revision_id, &model_hash, &vector)
                    .map_err(|error| error.to_string())?;
                repository::upsert_grant(
                    &transaction,
                    &principal,
                    &format!("tool-{index}"),
                    "user",
                    &principal,
                )
                .map_err(|error| error.to_string())?;
            }
            repository::bump_epochs(&transaction, false, true, false)
                .map_err(|error| error.to_string())?;
            transaction.commit().map_err(crate::database_error)?;
            Ok(())
        })
        .expect("register ledger");

    let service = Arc::new(ToolSelectionService::new(
        writer.clone(),
        embedding,
        Arc::new(HashReranker::new()),
        Arc::new(UnconfiguredExtractor),
        backend,
        f64::NEG_INFINITY,
    ));
    (writer, service)
}
pub(crate) async fn serve_with(
    writer: Arc<SqliteWriter>,
    service: Arc<ToolSelectionService>,
) -> D5 {
    let directory = tempfile::tempdir().expect("tempdir");
    let token = token_value();
    let token_path = directory.path().join("token");
    std::fs::write(&token_path, token.as_bytes()).expect("write token");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&token_path, std::fs::Permissions::from_mode(0o600))
            .expect("mode");
    }
    let config = McpServerConfig {
        enabled: true,
        port: 0,
        token_file: Some(token_path),
        project_id: None,
    };
    let server = super::start(service, writer.clone(), config)
        .await
        .expect("start");
    let base = format!("http://127.0.0.1:{}/mcp", server.port());
    D5 {
        server,
        base,
        token,
        client: reqwest::Client::new(),
        writer,
        _directory: directory,
    }
}
pub(crate) async fn harness(count: usize) -> D5 {
    let (writer, service) = ledger(count);
    serve_with(writer, service).await
}
impl D5 {
    async fn post(&self, body: &Value, session: Option<&str>) -> reqwest::Response {
        let mut request = self
            .client
            .post(&self.base)
            .header("Authorization", format!("Bearer {}", self.token))
            .header("Accept", ACCEPT_BOTH)
            .json(body);
        if let Some(session) = session {
            request = request.header("Mcp-Session-Id", session);
        }
        request.send().await.expect("request")
    }

    pub(crate) async fn rpc(
        &self,
        body: &Value,
        session: Option<&str>,
    ) -> (reqwest::StatusCode, Value) {
        let response = self.post(body, session).await;
        let status = response.status();
        let value = response.json::<Value>().await.unwrap_or(Value::Null);
        (status, value)
    }

    pub(crate) async fn initialize(&self, id: i64) -> String {
        let response = self
            .client
            .post(&self.base)
            .header("Authorization", format!("Bearer {}", self.token))
            .header("Accept", ACCEPT_BOTH)
            .json(&json!({
                "jsonrpc": "2.0",
                "id": id,
                "method": "initialize",
                "params": {
                    "protocolVersion": "2025-06-18",
                    "capabilities": {},
                    "clientInfo": { "name": "test", "version": "1" }
                }
            }))
            .send()
            .await
            .expect("initialize");
        assert_eq!(response.status(), reqwest::StatusCode::OK);
        let session = response
            .headers()
            .get("mcp-session-id")
            .and_then(|value| value.to_str().ok())
            .expect("session id")
            .to_string();
        let value: Value = response.json().await.expect("json");
        assert_eq!(
            value.pointer("/result/protocolVersion"),
            Some(&json!("2025-06-18"))
        );
        assert_eq!(
            value.pointer("/result/capabilities/tools/listChanged"),
            Some(&json!(false))
        );
        session
    }

    pub(crate) async fn ready_session(&self) -> String {
        let session = self.initialize(1).await;
        let response = self
            .post(
                &json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }),
                Some(&session),
            )
            .await;
        assert_eq!(response.status(), reqwest::StatusCode::ACCEPTED);
        session
    }

    pub(crate) async fn ready_role_session(&self, root_id: &str) -> String {
        let url = format!("{}?rrRoot={root_id}", self.base);
        let response = self
            .client
            .post(url)
            .header("Authorization", format!("Bearer {}", self.token))
            .header("Accept", ACCEPT_BOTH)
            .json(&json!({
                "jsonrpc":"2.0","id":1,"method":"initialize",
                "params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"role-test","version":"1"}}
            }))
            .send()
            .await
            .expect("role initialize");
        assert_eq!(response.status(), reqwest::StatusCode::OK);
        let session = response
            .headers()
            .get("mcp-session-id")
            .and_then(|value| value.to_str().ok())
            .expect("role session id")
            .to_string();
        let response = self
            .post(
                &json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
                Some(&session),
            )
            .await;
        assert_eq!(response.status(), reqwest::StatusCode::ACCEPTED);
        session
    }

    pub(crate) async fn call(&self, id: i64, name: &str, arguments: Value, session: &str) -> Value {
        let (status, value) = self
            .rpc(
                &json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "method": "tools/call",
                    "params": { "name": name, "arguments": arguments }
                }),
                Some(session),
            )
            .await;
        assert_eq!(status, reqwest::StatusCode::OK);
        value
    }

    pub(crate) async fn envelope(
        &self,
        id: i64,
        name: &str,
        arguments: Value,
        session: &str,
    ) -> Value {
        let value = self.call(id, name, arguments, session).await;
        let text = value
            .pointer("/result/content/0/text")
            .and_then(Value::as_str)
            .expect("content text");
        serde_json::from_str(text).expect("gateway envelope")
    }
}
#[tokio::test]
pub(crate) async fn h01_tools_list_is_exactly_three_definitions_at_scale() {
    let harness = harness(1500).await;
    let session = harness.ready_session().await;
    let (status, value) = harness
        .rpc(
            &json!({ "jsonrpc": "2.0", "id": 7, "method": "tools/list" }),
            Some(&session),
        )
        .await;
    assert_eq!(status, reqwest::StatusCode::OK);
    let tools = value
        .pointer("/result/tools")
        .and_then(Value::as_array)
        .expect("tools");
    assert_eq!(tools.len(), 3);
    let names: Vec<&str> = tools
        .iter()
        .filter_map(|tool| tool.get("name").and_then(Value::as_str))
        .collect();
    assert_eq!(
        names,
        vec!["tools_search", "tools_describe", "tools_invoke"]
    );
    let body = serde_json::to_vec(&value).expect("serialize");
    assert!(body.len() < 32 * 1024);
    // The ledger schema is never blended into tools/list.
    let text = String::from_utf8(body).expect("utf8");
    assert!(!text.contains("tool_0100"));
}
#[tokio::test]
pub(crate) async fn h02_authentication_origin_and_host_are_enforced() {
    let harness = harness(1).await;
    let body = json!({ "jsonrpc": "2.0", "id": 1, "method": "ping" });

    // No credentials, wrong scheme, wrong token.
    let response = harness
        .client
        .post(&harness.base)
        .header("Accept", ACCEPT_BOTH)
        .json(&body)
        .send()
        .await
        .expect("no auth");
    assert_eq!(response.status(), reqwest::StatusCode::UNAUTHORIZED);

    let response = harness
        .client
        .post(&harness.base)
        .header("Authorization", format!("Bearer {}", "z".repeat(43)))
        .header("Accept", ACCEPT_BOTH)
        .json(&body)
        .send()
        .await
        .expect("bad token");
    assert_eq!(response.status(), reqwest::StatusCode::UNAUTHORIZED);

    // The auth scheme is case-insensitive: authentication passes and only the missing session
    // produces an HTTP error.
    let response = harness
        .client
        .post(&harness.base)
        .header("Authorization", format!("bearer {}", harness.token))
        .header("Accept", ACCEPT_BOTH)
        .json(&body)
        .send()
        .await
        .expect("lowercase scheme");
    assert_eq!(response.status(), reqwest::StatusCode::BAD_REQUEST);

    // Origin is refused.
    let response = harness
        .client
        .post(&harness.base)
        .header("Authorization", format!("Bearer {}", harness.token))
        .header("Accept", ACCEPT_BOTH)
        .header("Origin", "https://evil.example")
        .json(&body)
        .send()
        .await
        .expect("origin");
    assert_eq!(response.status(), reqwest::StatusCode::FORBIDDEN);

    // Host mismatch is refused.
    let response = harness
        .client
        .post(&harness.base)
        .header("Authorization", format!("Bearer {}", harness.token))
        .header("Accept", ACCEPT_BOTH)
        .header("Host", "evil.example")
        .json(&body)
        .send()
        .await
        .expect("host");
    assert_eq!(response.status(), reqwest::StatusCode::FORBIDDEN);

    // Every method requires the same bearer authentication, not just POST.
    let response = harness
        .client
        .get(&harness.base)
        .send()
        .await
        .expect("get no auth");
    assert_eq!(response.status(), reqwest::StatusCode::UNAUTHORIZED);
    let response = harness
        .client
        .delete(&harness.base)
        .send()
        .await
        .expect("delete no auth");
    assert_eq!(response.status(), reqwest::StatusCode::UNAUTHORIZED);
    let response = harness
        .client
        .get(&harness.base)
        .header("Authorization", format!("Bearer {}", harness.token))
        .header("Origin", "https://evil.example")
        .send()
        .await
        .expect("get origin");
    assert_eq!(response.status(), reqwest::StatusCode::FORBIDDEN);

    // A rejected request never reaches the ledger: no decision row was written.
    let decisions: i64 = harness
        .writer
        .read_serialized(|connection| {
            connection
                .query_row("SELECT COUNT(*) FROM tool_selection_decisions", [], |row| {
                    row.get(0)
                })
                .map_err(|error| error.to_string())
        })
        .expect("decisions");
    assert_eq!(decisions, 0);
}
