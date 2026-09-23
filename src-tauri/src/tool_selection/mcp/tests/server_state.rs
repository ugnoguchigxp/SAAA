use super::*;
pub(crate) const PRINCIPAL: &str = "P-D4";
// ---------------------------------------------------------------------------------------------
// Test HTTP server
// ---------------------------------------------------------------------------------------------

#[derive(Default)]
pub(crate) struct ServerState {
    pub(crate) list_pages: Mutex<Vec<Value>>,
    pub(crate) call_result: Mutex<Value>,
    pub(crate) call_count: AtomicUsize,
    pub(crate) list_count: AtomicUsize,
    pub(crate) session: Mutex<Option<String>>,
    pub(crate) sse: AtomicBool,
    pub(crate) get_supported: AtomicBool,
    pub(crate) protocol_version: Mutex<String>,
    pub(crate) tools_capability: AtomicBool,
    pub(crate) disconnect_after_call: AtomicBool,
    pub(crate) delete_called: AtomicBool,
    pub(crate) initialized_seen: AtomicBool,
    pub(crate) injected_bad_page: Mutex<Option<usize>>,
    pub(crate) cursor_map: Mutex<HashMap<String, usize>>,
    pub(crate) calls_while_uninitialized: AtomicUsize,
    pub(crate) unauthorized: AtomicBool,
    pub(crate) emit_progress: AtomicBool,
    pub(crate) server_request: AtomicBool,
    pub(crate) unsupported_reply: Mutex<Option<i64>>,
    pub(crate) gate_list: AtomicBool,
    pub(crate) list_received: Arc<tokio::sync::Notify>,
    pub(crate) list_release: Arc<tokio::sync::Notify>,
    pub(crate) gate_call: AtomicBool,
    pub(crate) call_received: Arc<tokio::sync::Notify>,
    pub(crate) call_release: Arc<tokio::sync::Notify>,
}
pub(crate) struct MockServer {
    pub(crate) address: SocketAddr,
    pub(crate) task: tokio::task::JoinHandle<()>,
}
impl MockServer {
    pub(crate) async fn start(state: Arc<ServerState>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let address = listener.local_addr().expect("addr");
        let shared = state.clone();
        let task = tokio::spawn(async move {
            loop {
                let Ok((mut socket, _)) = listener.accept().await else {
                    break;
                };
                let state = shared.clone();
                tokio::spawn(async move {
                    let _ = serve(&mut socket, &state).await;
                });
            }
        });
        MockServer { address, task }
    }

    pub(crate) fn url(&self) -> String {
        format!("http://{}/mcp", self.address)
    }
}
impl Drop for MockServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}
pub(crate) fn default_state() -> Arc<ServerState> {
    let state = Arc::new(ServerState {
        protocol_version: Mutex::new(super::MCP_PROTOCOL_VERSION.to_string()),
        tools_capability: AtomicBool::new(true),
        call_result: Mutex::new(json!({
            "content": [{ "type": "text", "text": "ok" }],
            "isError": false,
        })),
        ..ServerState::default()
    });
    // One page with two tools by default.
    state.list_pages.lock().unwrap().push(json!({
        "tools": [
            {
                "name": "search",
                "description": "Search notes.",
                "inputSchema": { "type": "object", "properties": { "q": { "type": "string" } }, "required": ["q"] }
            },
            {
                "name": "other",
                "description": "Another tool.",
                "inputSchema": { "type": "object" }
            }
        ]
    }));
    state
}
pub(crate) async fn read_request(
    socket: &mut tokio::net::TcpStream,
) -> Option<(String, HashMap<String, String>, Vec<u8>)> {
    let mut buffer = Vec::new();
    let mut chunk = [0_u8; 4096];
    let header_end;
    loop {
        let read = socket.read(&mut chunk).await.ok()?;
        if read == 0 {
            return None;
        }
        buffer.extend_from_slice(&chunk[..read]);
        if let Some(position) = find_header_end(&buffer) {
            header_end = position;
            break;
        }
        if buffer.len() > 64 * 1024 {
            return None;
        }
    }
    let header_text = String::from_utf8_lossy(&buffer[..header_end]).to_string();
    let mut lines = header_text.split("\r\n");
    let request_line = lines.next().unwrap_or_default().to_string();
    let mut headers = HashMap::new();
    for line in lines {
        if let Some((key, value)) = line.split_once(':') {
            headers.insert(key.trim().to_ascii_lowercase(), value.trim().to_string());
        }
    }
    let content_length: usize = headers
        .get("content-length")
        .and_then(|value| value.parse().ok())
        .unwrap_or(0);
    let body_start = header_end + 4;
    while buffer.len() < body_start + content_length {
        let read = socket.read(&mut chunk).await.ok()?;
        if read == 0 {
            break;
        }
        buffer.extend_from_slice(&chunk[..read]);
    }
    let body = buffer
        .get(body_start..body_start + content_length)
        .map(|bytes| bytes.to_vec())
        .unwrap_or_default();
    Some((request_line, headers, body))
}
pub(crate) fn find_header_end(buffer: &[u8]) -> Option<usize> {
    buffer.windows(4).position(|window| window == b"\r\n\r\n")
}
pub(crate) async fn write_response(
    socket: &mut tokio::net::TcpStream,
    status: &str,
    content_type: &str,
    body: &[u8],
) -> std::io::Result<()> {
    let head = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    socket.write_all(head.as_bytes()).await?;
    socket.write_all(body).await?;
    socket.flush().await
}
pub(crate) async fn write_sse(
    socket: &mut tokio::net::TcpStream,
    message: &Value,
) -> std::io::Result<()> {
    write_sse_messages(socket, std::slice::from_ref(message)).await
}
pub(crate) async fn write_sse_messages(
    socket: &mut tokio::net::TcpStream,
    messages: &[Value],
) -> std::io::Result<()> {
    let head = "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n";
    socket.write_all(head.as_bytes()).await?;
    for message in messages {
        let event = format!("data: {message}\n\n");
        socket.write_all(event.as_bytes()).await?;
    }
    socket.flush().await
}
pub(crate) async fn serve(
    socket: &mut tokio::net::TcpStream,
    state: &Arc<ServerState>,
) -> std::io::Result<()> {
    let Some((request_line, headers, body)) = read_request(socket).await else {
        return Ok(());
    };
    let method = request_line
        .split(' ')
        .next()
        .unwrap_or_default()
        .to_string();
    if method == "GET" {
        if state.get_supported.load(Ordering::SeqCst) {
            if state.server_request.load(Ordering::SeqCst) {
                // An unsupported server request must receive a JSON-RPC method-not-found reply.
                let request = json!({
                    "jsonrpc": "2.0",
                    "id": "srv-1",
                    "method": "sampling/createMessage",
                    "params": {}
                });
                return write_sse(socket, &request).await;
            }
            let notification = json!({
                "jsonrpc": "2.0",
                "method": "notifications/tools/list_changed"
            });
            return write_sse(socket, &notification).await;
        }
        return write_response(socket, "405 Method Not Allowed", "text/plain", b"").await;
    }
    if method == "DELETE" {
        state.delete_called.store(true, Ordering::SeqCst);
        return write_response(socket, "200 OK", "application/json", b"{}").await;
    }
    let request: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
    if state.unauthorized.load(Ordering::SeqCst) {
        return write_response(socket, "401 Unauthorized", "text/plain", b"").await;
    }
    let rpc_method = request
        .get("method")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let id = request.get("id").cloned();
    let session_matches = {
        let session = state.session.lock().unwrap().clone();
        match session {
            None => true,
            Some(session) => {
                headers.get("mcp-session-id").map(String::as_str) == Some(session.as_str())
            }
        }
    };
    match rpc_method {
        "initialize" => {
            let mut result = json!({
                "protocolVersion": state.protocol_version.lock().unwrap().clone(),
                "capabilities": {},
                "serverInfo": { "name": "mock", "version": "1" },
            });
            if state.tools_capability.load(Ordering::SeqCst) {
                result["capabilities"]["tools"] = json!({});
            }
            let mut head = String::from(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nConnection: close\r\n",
            );
            if let Some(session) = state.session.lock().unwrap().clone() {
                head.push_str(&format!("Mcp-Session-Id: {session}\r\n"));
            }
            let body = json!({ "jsonrpc": "2.0", "id": id, "result": result }).to_string();
            head.push_str(&format!("Content-Length: {}\r\n\r\n", body.len()));
            socket.write_all(head.as_bytes()).await?;
            socket.write_all(body.as_bytes()).await?;
            socket.flush().await
        }
        "notifications/initialized" => {
            state.initialized_seen.store(true, Ordering::SeqCst);
            return write_response(socket, "202 Accepted", "application/json", b"").await;
        }
        "tools/list" => {
            if !session_matches {
                return write_response(socket, "404 Not Found", "text/plain", b"").await;
            }
            if !state.initialized_seen.load(Ordering::SeqCst) {
                state
                    .calls_while_uninitialized
                    .fetch_add(1, Ordering::SeqCst);
            }
            let index = state.list_count.fetch_add(1, Ordering::SeqCst);
            let bad = state.injected_bad_page.lock().unwrap().as_ref().copied();
            if bad == Some(index) {
                return write_response(socket, "200 OK", "application/json", b"{not json").await;
            }
            if state.gate_list.load(Ordering::SeqCst) && index == 0 {
                // A deterministic barrier: the test observes the request before it is answered.
                state.list_received.notify_one();
                state.list_release.notified().await;
            }
            let cursor = request
                .get("params")
                .and_then(|params| params.get("cursor"))
                .and_then(Value::as_str)
                .map(str::to_string);
            let pages = state.list_pages.lock().unwrap().clone();
            let page_index = match cursor.as_deref() {
                None => 0_usize,
                Some(cursor) => state
                    .cursor_map
                    .lock()
                    .unwrap()
                    .get(cursor)
                    .copied()
                    .unwrap_or(0),
            };
            let mut result = pages
                .get(page_index)
                .cloned()
                .unwrap_or_else(|| json!({ "tools": [] }));
            let explicit = result
                .get("nextCursor")
                .and_then(Value::as_str)
                .map(str::to_string);
            let next = explicit
                .or_else(|| (page_index + 1 < pages.len()).then(|| format!("cursor-{page_index}")));
            match next {
                Some(next) => {
                    result["nextCursor"] = json!(next);
                    state
                        .cursor_map
                        .lock()
                        .unwrap()
                        .insert(next, page_index + 1);
                }
                None => {
                    if let Some(object) = result.as_object_mut() {
                        object.remove("nextCursor");
                    }
                }
            }
            let message = json!({ "jsonrpc": "2.0", "id": id.clone(), "result": result });
            if state.sse.load(Ordering::SeqCst) {
                return write_sse(socket, &message).await;
            }
            return write_response(
                socket,
                "200 OK",
                "application/json",
                message.to_string().as_bytes(),
            )
            .await;
        }
        "tools/call" => {
            state.call_count.fetch_add(1, Ordering::SeqCst);
            if state.gate_call.load(Ordering::SeqCst) {
                state.call_received.notify_one();
                state.call_release.notified().await;
            }
            if state.disconnect_after_call.load(Ordering::SeqCst) {
                // Drop the socket without a response to model an indeterminate outcome.
                let _ = socket.shutdown().await;
                return Ok(());
            }
            let result = state.call_result.lock().unwrap().clone();
            let message = json!({ "jsonrpc": "2.0", "id": id.clone(), "result": result });
            if state.sse.load(Ordering::SeqCst) {
                if state.emit_progress.load(Ordering::SeqCst) {
                    let progress = json!({
                        "jsonrpc": "2.0",
                        "method": "notifications/progress",
                        "params": { "progress": 1, "total": 2 }
                    });
                    return write_sse_messages(socket, &[progress, message]).await;
                }
                return write_sse(socket, &message).await;
            }
            return write_response(
                socket,
                "200 OK",
                "application/json",
                message.to_string().as_bytes(),
            )
            .await;
        }
        "notifications/cancelled" => {
            return write_response(socket, "202 Accepted", "application/json", b"").await;
        }
        _ => {
            // A response to a server request carries no method.
            if request.get("result").is_some() || request.get("error").is_some() {
                if let Some(code) = request.pointer("/error/code").and_then(Value::as_i64) {
                    *state.unsupported_reply.lock().unwrap() = Some(code);
                }
                return write_response(socket, "202 Accepted", "application/json", b"").await;
            }
            return write_response(socket, "200 OK", "application/json", b"{}").await;
        }
    }
}
// ---------------------------------------------------------------------------------------------
// Ledger harness
// ---------------------------------------------------------------------------------------------

pub(crate) struct Harness {
    pub(crate) writer: Arc<SqliteWriter>,
    pub(crate) manager: Arc<McpManager>,
    pub(crate) service: ToolSelectionService,
    pub(crate) principal: String,
}
pub(crate) fn hash_embedding() -> Arc<dyn EmbeddingProvider> {
    Arc::new(HashEmbeddingAdapter)
}
/// Adapter so the harness can hand the manager an `EmbeddingProvider` without depending on the
/// test-only `HashEmbedding` type name.
struct HashEmbeddingAdapter;
#[async_trait::async_trait]
impl EmbeddingProvider for HashEmbeddingAdapter {
    fn model_hash(&self) -> &str {
        "test-d4-hash"
    }
    fn dimension(&self) -> usize {
        16
    }
    async fn embed(
        &self,
        _kind: EmbedKind,
        texts: &[String],
    ) -> Result<Vec<Vec<f32>>, InferenceError> {
        Ok(texts
            .iter()
            .map(|text| {
                let mut vector = vec![0.0_f32; 16];
                for (index, byte) in text.bytes().enumerate() {
                    vector[(index + byte as usize) % 16] += 1.0;
                }
                let norm = vector.iter().map(|v| v * v).sum::<f32>().sqrt();
                if norm > 0.0 {
                    for value in &mut vector {
                        *value /= norm;
                    }
                }
                vector
            })
            .collect())
    }
}
impl Harness {
    pub(crate) fn new(server: &MockServer, grants: Vec<McpGrantSpec>) -> Self {
        Self::new_with_embedder(server, grants, true)
    }

    pub(crate) fn new_with_embedder(
        server: &MockServer,
        grants: Vec<McpGrantSpec>,
        with_embedder: bool,
    ) -> Self {
        let connection = Connection::open_in_memory().expect("in-memory");
        crate::persistence::schema::initialize_database(&connection).expect("schema");
        let writer = Arc::new(SqliteWriter::from_connection(connection));
        writer
            .write(|connection| {
                connection
                    .execute(
                        "INSERT OR IGNORE INTO conversations(id, title, task_mode, created_at, updated_at)
                         VALUES ('conversation-d4', 'D4', 'conversation', '1', '1')",
                        [],
                    )
                    .map_err(|error| error.to_string())?;
                Ok(())
            })
            .expect("conversation");
        // The principal is created through the settings document so the manager and the service
        // share one id.
        let principal =
            crate::tool_selection::service::ensure_principal(&writer).expect("principal");
        let source = McpSourceSpec {
            id: "mcp-test".to_string(),
            url: server.url(),
            enabled: true,
            bearer_token_env: None,
            grants,
        };
        let sources = McpSources {
            sources: vec![source],
        };
        let manager = McpManager::new(
            writer.clone(),
            principal.clone(),
            None,
            sources,
            with_embedder.then(hash_embedding),
            None,
        );
        let router = Arc::new(BackendRouter::new(
            Arc::new(FixtureBackend::new()),
            Arc::new(McpBackend::new(manager.clone())),
            Arc::new(crate::records::backend::RecordsBackend::new(writer.clone())),
        ));
        let embedding: Arc<dyn EmbeddingProvider> = if with_embedder {
            hash_embedding()
        } else {
            Arc::new(crate::tool_selection::inference::UnavailableEmbedding)
        };
        let mut service = ToolSelectionService::new(
            writer.clone(),
            embedding,
            Arc::new(FixedReranker::new(&[])),
            Arc::new(UnconfiguredExtractor),
            router,
            f64::NEG_INFINITY,
        );
        service.set_discovery_configured(true);
        service.set_mcp_manager(manager.clone());
        Harness {
            writer,
            manager,
            service,
            principal,
        }
    }

    pub(crate) fn context(&self) -> RequestContext {
        RequestContext::new(&self.principal, "conversation-d4").with_run(Some("run-d4".to_string()))
    }

    pub(crate) fn count(&self, table: &str) -> i64 {
        self.writer
            .read_serialized(|connection| {
                connection
                    .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                        row.get(0)
                    })
                    .map_err(|error| error.to_string())
            })
            .expect("count")
    }

    pub(crate) fn epochs(&self) -> repository::Epochs {
        self.writer
            .read_serialized(|connection| {
                repository::epochs(connection).map_err(|error| error.to_string())
            })
            .expect("epochs")
    }
}
