#![cfg(test)]
//! D4 deterministic acceptance tests. A real HTTP/1.1 test server speaks the MCP Streamable
//! transport (JSON and SSE) so transport, sync, invocation and continuation are exercised over a
//! socket, not against a mocked trait. No external network service is used.

use rusqlite::Connection;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

use super::config::{McpGrantScope, McpGrantSpec, McpSourceSpec, McpSources};
use super::descriptors;
use super::manager::McpManager;
use super::results;
use crate::persistence::SqliteWriter;
use crate::tool_selection::backends::mcp::McpBackend;
use crate::tool_selection::backends::router::BackendRouter;
use crate::tool_selection::backends::FixtureBackend;
use crate::tool_selection::contracts::now_ms;
use crate::tool_selection::extraction::UnconfiguredExtractor;
use crate::tool_selection::inference::{
    EmbedKind, EmbeddingProvider, FixedReranker, InferenceError,
};
use crate::tool_selection::repository;
use crate::tool_selection::service::ToolSelectionService;
use crate::tool_selection::{RequestContext, Scenario};

const PRINCIPAL: &str = "P-D4";

// ---------------------------------------------------------------------------------------------
// Test HTTP server
// ---------------------------------------------------------------------------------------------

#[derive(Default)]
struct ServerState {
    list_pages: Mutex<Vec<Value>>,
    call_result: Mutex<Value>,
    call_count: AtomicUsize,
    list_count: AtomicUsize,
    session: Mutex<Option<String>>,
    sse: AtomicBool,
    get_supported: AtomicBool,
    protocol_version: Mutex<String>,
    tools_capability: AtomicBool,
    disconnect_after_call: AtomicBool,
    delete_called: AtomicBool,
    initialized_seen: AtomicBool,
    injected_bad_page: Mutex<Option<usize>>,
    cursor_map: Mutex<HashMap<String, usize>>,
    calls_while_uninitialized: AtomicUsize,
    unauthorized: AtomicBool,
    emit_progress: AtomicBool,
    server_request: AtomicBool,
    unsupported_reply: Mutex<Option<i64>>,
    gate_list: AtomicBool,
    list_received: Arc<tokio::sync::Notify>,
    list_release: Arc<tokio::sync::Notify>,
    gate_call: AtomicBool,
    call_received: Arc<tokio::sync::Notify>,
    call_release: Arc<tokio::sync::Notify>,
}

struct MockServer {
    address: SocketAddr,
    task: tokio::task::JoinHandle<()>,
}

impl MockServer {
    async fn start(state: Arc<ServerState>) -> Self {
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

    fn url(&self) -> String {
        format!("http://{}/mcp", self.address)
    }
}

impl Drop for MockServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

fn default_state() -> Arc<ServerState> {
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

async fn read_request(
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

fn find_header_end(buffer: &[u8]) -> Option<usize> {
    buffer.windows(4).position(|window| window == b"\r\n\r\n")
}

async fn write_response(
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

async fn write_sse(socket: &mut tokio::net::TcpStream, message: &Value) -> std::io::Result<()> {
    write_sse_messages(socket, std::slice::from_ref(message)).await
}

async fn write_sse_messages(
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

async fn serve(
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

struct Harness {
    writer: Arc<SqliteWriter>,
    manager: Arc<McpManager>,
    service: ToolSelectionService,
    principal: String,
}

fn hash_embedding() -> Arc<dyn EmbeddingProvider> {
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
    fn new(server: &MockServer, grants: Vec<McpGrantSpec>) -> Self {
        Self::new_with_embedder(server, grants, true)
    }

    fn new_with_embedder(
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

    fn context(&self) -> RequestContext {
        RequestContext::new(&self.principal, "conversation-d4").with_run(Some("run-d4".to_string()))
    }

    fn count(&self, table: &str) -> i64 {
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

    fn epochs(&self) -> repository::Epochs {
        self.writer
            .read_serialized(|connection| {
                repository::epochs(connection).map_err(|error| error.to_string())
            })
            .expect("epochs")
    }
}

fn user_grant(tool_name: &str) -> McpGrantSpec {
    McpGrantSpec {
        tool_name: tool_name.to_string(),
        scope_kind: McpGrantScope::User,
        project_id: None,
    }
}

fn scenario() -> Scenario {
    Scenario {
        intent: "search notes".to_string(),
        operation: crate::tool_selection::Operation::Search,
        object_type: crate::tool_selection::ObjectType::Document,
        phase: crate::tool_selection::Phase::Discover,
        input_kind: crate::tool_selection::InputKind::Text,
    }
}

// ---------------------------------------------------------------------------------------------
// T01 configuration
// ---------------------------------------------------------------------------------------------

#[test]
fn t01_config_rejects_bad_documents() {
    let cases: Vec<(&str, &[u8], Option<&str>)> = vec![
        ("ok", br#"{"formatVersion":1,"sources":[]}"#, None),
        ("unknown field", br#"{"formatVersion":1,"sources":[],"extra":1}"#, Some("mcp-sources-invalid-json")),
        ("bad version", br#"{"formatVersion":2,"sources":[]}"#, Some("mcp-sources-version-unknown")),
        ("bad id", br#"{"formatVersion":1,"sources":[{"id":"Bad","url":"https://x/mcp"}]}"#, Some("mcp-sources-id-invalid")),
        (
            "duplicate id",
            br#"{"formatVersion":1,"sources":[{"id":"a","url":"https://x/mcp"},{"id":"a","url":"https://y/mcp"}]}"#,
            Some("mcp-sources-duplicate-id"),
        ),
        ("insecure http", br#"{"formatVersion":1,"sources":[{"id":"a","url":"http://example.com/mcp"}]}"#, Some("mcp-sources-url-insecure")),
        ("userinfo", br#"{"formatVersion":1,"sources":[{"id":"a","url":"https://u:p@x/mcp"}]}"#, Some("mcp-sources-url-invalid")),
        ("fragment", br#"{"formatVersion":1,"sources":[{"id":"a","url":"https://x/mcp#f"}]}"#, Some("mcp-sources-url-invalid")),
        (
            "project grant without id",
            br#"{"formatVersion":1,"sources":[{"id":"a","url":"https://x/mcp","grants":[{"toolName":"t","scopeKind":"project"}]}]}"#,
            Some("mcp-sources-grant-invalid"),
        ),
    ];
    for (label, bytes, expected) in cases {
        let result = McpSources::parse(bytes);
        assert_eq!(result.as_ref().err().copied(), expected, "case {label}");
        if expected.is_none() {
            assert!(result.is_ok(), "case {label}");
        }
    }
}

#[test]
fn t01_loopback_http_is_allowed_and_endpoint_hash_is_stable() {
    let parsed = McpSources::parse(
        br#"{"formatVersion":1,"sources":[{"id":"a","url":"http://127.0.0.1:8787/mcp","bearerTokenEnv":"SAAA_TEST_TOKEN"}]}"#,
    )
    .expect("valid");
    let source = &parsed.sources[0];
    assert_eq!(source.bearer_token_env.as_deref(), Some("SAAA_TEST_TOKEN"));
    // The endpoint hash is stable and drops the query so a token in a query parameter cannot be
    // embedded in a revision binding.
    let with_query = super::config::endpoint_hash("http://127.0.0.1:8787/mcp?token=secret");
    let without_query = super::config::endpoint_hash("http://127.0.0.1:8787/mcp");
    assert_eq!(with_query, without_query);
    assert_eq!(source.endpoint_hash().len(), 64);
}

#[test]
fn t01_missing_token_environment_reports_unavailable() {
    let source = McpSourceSpec {
        id: "a".to_string(),
        url: "https://example.com/mcp".to_string(),
        enabled: true,
        bearer_token_env: Some("SAAA_D4_MISSING_TOKEN_FOR_TEST".to_string()),
        grants: Vec::new(),
    };
    assert!(source.resolve_token().is_none());
}

// ---------------------------------------------------------------------------------------------
// T03 descriptors and stable ids
// ---------------------------------------------------------------------------------------------

fn descriptor(name: &str, description: &str) -> Value {
    json!({
        "name": name,
        "description": description,
        "inputSchema": { "type": "object", "properties": { "q": { "type": "string" } }, "required": ["q"] }
    })
}

#[test]
fn t03_ids_are_stable_and_source_scoped() {
    let endpoint = "e".repeat(64);
    let a =
        descriptors::normalize_tool("src-a", &endpoint, &descriptor("search", "one")).expect("a");
    let b =
        descriptors::normalize_tool("src-b", &endpoint, &descriptor("search", "one")).expect("b");
    assert_ne!(
        a.tool_id, b.tool_id,
        "same name in different sources is a different tool"
    );
    assert!(a.tool_id.starts_with("mcpt_"));
    assert!(a.revision_id.starts_with("mcpr_"));

    // Key order changes do not move the revision id.
    let reordered = json!({
        "description": "one",
        "name": "search",
        "inputSchema": { "required": ["q"], "type": "object", "properties": { "q": { "type": "string" } } }
    });
    let c = descriptors::normalize_tool("src-a", &endpoint, &reordered).expect("c");
    assert_eq!(a.revision_id, c.revision_id);
    assert_eq!(a.tool_id, c.tool_id);
}

#[test]
fn t03_a_to_b_to_a_returns_to_a_and_detects_changes() {
    let endpoint = "e".repeat(64);
    let a = descriptors::normalize_tool("src", &endpoint, &descriptor("x", "A")).expect("a");
    let b = descriptors::normalize_tool("src", &endpoint, &descriptor("x", "B")).expect("b");
    let a_again = descriptors::normalize_tool("src", &endpoint, &descriptor("x", "A")).expect("a2");
    assert_ne!(a.revision_id, b.revision_id);
    assert_eq!(a.revision_id, a_again.revision_id);
    assert_eq!(a.tool_id, b.tool_id);

    let other_endpoint =
        descriptors::normalize_tool("src", &"f".repeat(64), &descriptor("x", "A")).expect("e");
    assert_ne!(a.revision_id, other_endpoint.revision_id);
}

#[test]
fn t03_unsupported_schema_fails_instead_of_substituting_empty() {
    let endpoint = "e".repeat(64);
    // A non-object schema is rejected; the sync must not substitute `{}`.
    let bad = json!({ "name": "x", "inputSchema": 5 });
    assert!(descriptors::normalize_tool("src", &endpoint, &bad).is_err());
}

#[test]
fn t03_descriptor_hash_covers_annotations_and_management() {
    let endpoint = "e".repeat(64);
    let base = descriptor("x", "A");
    let annotated = json!({
        "name": "x",
        "description": "A",
        "inputSchema": { "type": "object", "properties": { "q": { "type": "string" } }, "required": ["q"] },
        "annotations": { "readOnlyHint": true },
        "_meta": { "saaa": { "effect": "read", "operations": ["read"] } }
    });
    let a = descriptors::normalize_tool("src", &endpoint, &base).expect("a");
    let b = descriptors::normalize_tool("src", &endpoint, &annotated).expect("b");
    assert_ne!(a.revision_id, b.revision_id);
    assert_eq!(b.entry.effect, "read");
    assert_eq!(b.entry.operations, vec!["read".to_string()]);
}

// ---------------------------------------------------------------------------------------------
// T04 transport and session
// ---------------------------------------------------------------------------------------------

#[tokio::test]
async fn t04_json_and_sse_tools_call_both_work() {
    for sse in [false, true] {
        let state = default_state();
        state.sse.store(sse, Ordering::SeqCst);
        let server = MockServer::start(state.clone()).await;
        let harness = Harness::new(&server, vec![user_grant("search")]);
        harness.manager.sync_source("mcp-test").await.expect("sync");
        assert!(state.initialized_seen.load(Ordering::SeqCst));
        let tools = harness
            .writer
            .read_serialized(|c| {
                mcp_repo::published_tool_count(c, "mcp-test").map_err(|e| e.to_string())
            })
            .unwrap();
        assert_eq!(tools, 2);
    }
}

#[tokio::test]
async fn t04_session_404_triggers_reinitialize_for_the_next_operation() {
    let state = default_state();
    *state.session.lock().unwrap() = Some("sess-1".to_string());
    let server = MockServer::start(state.clone()).await;
    // First sync negotiates the session and succeeds.
    let harness = Harness::new(&server, vec![user_grant("search")]);
    harness.manager.sync_source("mcp-test").await.expect("sync");
    // The server forgets the session. The operation that observes the 404 fails; the next one
    // re-initializes and succeeds. An in-flight operation is never replayed.
    *state.session.lock().unwrap() = Some("sess-2".to_string());
    assert!(harness.manager.sync_source("mcp-test").await.is_err());
    assert!(harness.manager.sync_source("mcp-test").await.is_ok());
}

#[tokio::test]
async fn t04_get_405_is_not_an_error() {
    let state = default_state();
    state.get_supported.store(false, Ordering::SeqCst);
    let server = MockServer::start(state).await;
    let harness = Harness::new(&server, vec![user_grant("search")]);
    assert!(harness.manager.sync_source("mcp-test").await.is_ok());
}

// ---------------------------------------------------------------------------------------------
// T05 sync
// ---------------------------------------------------------------------------------------------

#[tokio::test]
async fn t05_failed_page_keeps_the_previous_snapshot_and_epoch() {
    let state = default_state();
    let server = MockServer::start(state.clone()).await;
    let harness = Harness::new(&server, vec![user_grant("search")]);
    harness
        .manager
        .sync_source("mcp-test")
        .await
        .expect("first sync");
    let epoch_before = harness.epochs().catalog;
    let count_before = harness.count("tool_selection_catalog");

    // Page index 1 (the second tools/list call) is invalid JSON.
    *state.injected_bad_page.lock().unwrap() = Some(1);
    assert!(harness.manager.sync_source("mcp-test").await.is_err());
    assert_eq!(harness.epochs().catalog, epoch_before);
    assert_eq!(harness.count("tool_selection_catalog"), count_before);
}

#[tokio::test]
async fn t05_same_content_resync_does_not_move_the_epoch() {
    let state = default_state();
    let server = MockServer::start(state).await;
    let harness = Harness::new(&server, vec![user_grant("search")]);
    harness
        .manager
        .sync_source("mcp-test")
        .await
        .expect("first");
    let epoch = harness.epochs().catalog;
    harness
        .manager
        .sync_source("mcp-test")
        .await
        .expect("second");
    assert_eq!(harness.epochs().catalog, epoch);
}

#[tokio::test]
async fn t05_changed_description_bumps_the_epoch_once() {
    let state = default_state();
    let server = MockServer::start(state.clone()).await;
    let harness = Harness::new(&server, vec![user_grant("search")]);
    harness
        .manager
        .sync_source("mcp-test")
        .await
        .expect("first");
    let epoch = harness.epochs().catalog;
    state.list_pages.lock().unwrap()[0]["tools"][0]["description"] =
        json!("Search notes, now with more detail.");
    harness
        .manager
        .sync_source("mcp-test")
        .await
        .expect("second");
    assert_eq!(harness.epochs().catalog, epoch + 1);
}

#[tokio::test]
async fn t05_disappearing_tool_is_disabled_and_reappears() {
    let state = default_state();
    let server = MockServer::start(state.clone()).await;
    let harness = Harness::new(&server, vec![user_grant("search")]);
    harness
        .manager
        .sync_source("mcp-test")
        .await
        .expect("first");
    state.list_pages.lock().unwrap()[0]["tools"] = json!([{
        "name": "search",
        "description": "Search notes.",
        "inputSchema": { "type": "object", "properties": { "q": { "type": "string" } }, "required": ["q"] }
    }]);
    harness
        .manager
        .sync_source("mcp-test")
        .await
        .expect("second");
    let other_id = descriptors::tool_id("mcp-test", "other");
    let enabled = harness
        .writer
        .read_serialized({
            let other_id = other_id.clone();
            move |c| mcp_repo::tool_enabled(c, &other_id).map_err(|e| e.to_string())
        })
        .unwrap();
    assert_eq!(enabled, Some(false));

    // The revision history is retained.
    let revisions: i64 = harness
        .writer
        .read_serialized(|connection| {
            connection
                .query_row(
                    "SELECT COUNT(*) FROM tool_selection_revisions WHERE tool_id = ?1",
                    rusqlite::params![other_id],
                    |row| row.get(0),
                )
                .map_err(|error| error.to_string())
        })
        .unwrap();
    assert_eq!(revisions, 1);
}

#[tokio::test]
async fn t05_cursor_loop_and_duplicate_names_fail_the_sync() {
    // Cursor loop: page 0 advertises cursor A, page 1 advertises cursor A again.
    let state = default_state();
    state.list_pages.lock().unwrap().clear();
    state.list_pages.lock().unwrap().push(json!({
        "tools": [{ "name": "a", "inputSchema": { "type": "object" } }],
        "nextCursor": "A"
    }));
    state.list_pages.lock().unwrap().push(json!({
        "tools": [{ "name": "b", "inputSchema": { "type": "object" } }],
        "nextCursor": "A"
    }));
    let server = MockServer::start(state).await;
    let harness = Harness::new(&server, vec![user_grant("a")]);
    assert_eq!(
        harness
            .manager
            .sync_source("mcp-test")
            .await
            .unwrap_err()
            .code,
        "sync-cursor-loop"
    );

    // Duplicate names in one snapshot.
    let state = default_state();
    state.list_pages.lock().unwrap()[0]["tools"] = json!([
        { "name": "dup", "inputSchema": { "type": "object" } },
        { "name": "dup", "inputSchema": { "type": "object" } }
    ]);
    let server = MockServer::start(state).await;
    let harness = Harness::new(&server, vec![user_grant("dup")]);
    assert_eq!(
        harness
            .manager
            .sync_source("mcp-test")
            .await
            .unwrap_err()
            .code,
        "sync-duplicate-name"
    );
}

#[tokio::test]
async fn t05_empty_page_with_cursor_is_allowed() {
    let state = default_state();
    state.list_pages.lock().unwrap().clear();
    state
        .list_pages
        .lock()
        .unwrap()
        .push(json!({ "tools": [], "nextCursor": "B" }));
    state.list_pages.lock().unwrap().push(json!({
        "tools": [{ "name": "a", "inputSchema": { "type": "object" } }]
    }));
    let server = MockServer::start(state).await;
    let harness = Harness::new(&server, vec![user_grant("a")]);
    harness.manager.sync_source("mcp-test").await.expect("sync");
    assert_eq!(harness.count("tool_selection_catalog"), 1);
}

// ---------------------------------------------------------------------------------------------
// T06 manager: grants and removal
// ---------------------------------------------------------------------------------------------

#[tokio::test]
async fn t06_import_alone_creates_no_grant_and_config_grants_are_managed() {
    let state = default_state();
    let server = MockServer::start(state).await;
    // No grants declared: the tools are imported but not authorized.
    let harness = Harness::new(&server, Vec::new());
    harness.manager.sync_source("mcp-test").await.expect("sync");
    let tool_id = descriptors::tool_id("mcp-test", "search");
    let principal = harness.principal.clone();
    let authorized = harness
        .writer
        .read_serialized({
            let principal = principal.clone();
            let tool_id = tool_id.clone();
            move |c| {
                repository::grant_exists(c, &principal, &tool_id, None).map_err(|e| e.to_string())
            }
        })
        .unwrap();
    assert!(!authorized, "import is never a grant");
    let eligibility = harness
        .writer
        .read_serialized(move |c| {
            repository::eligible_revisions(c, &principal, None, now_ms())
                .map(|rows| rows.len())
                .map_err(|e| e.to_string())
        })
        .unwrap();
    assert_eq!(eligibility, 0);
}

#[tokio::test]
async fn t06_removing_a_source_revokes_only_managed_grants() {
    let state = default_state();
    let server = MockServer::start(state).await;
    let harness = Harness::new(&server, vec![user_grant("search")]);
    harness.manager.sync_source("mcp-test").await.expect("sync");
    let search_id = descriptors::tool_id("mcp-test", "search");
    let other_id = descriptors::tool_id("mcp-test", "other");
    let principal = harness.principal.clone();
    // A manual grant outside the MCP config.
    harness
        .writer
        .write({
            let other_id = other_id.clone();
            let principal = principal.clone();
            move |connection| {
                repository::upsert_grant(connection, &principal, &other_id, "user", &principal)
                    .map_err(|e| e.to_string())?;
                Ok(())
            }
        })
        .unwrap();
    let acl_before = harness.epochs().acl;

    harness
        .manager
        .apply_sources(McpSources {
            sources: Vec::new(),
        })
        .await;

    let (search_granted, other_granted) = harness
        .writer
        .read_serialized({
            let principal = principal.clone();
            move |c| {
                let search = repository::grant_exists(c, &principal, &search_id, None)
                    .map_err(|e| e.to_string())?;
                let other = repository::grant_exists(c, &principal, &other_id, None)
                    .map_err(|e| e.to_string())?;
                Ok((search, other))
            }
        })
        .unwrap();
    assert!(!search_granted, "config grant is revoked");
    assert!(other_granted, "manual grant is preserved");
    assert!(harness.epochs().acl > acl_before, "acl epoch moves");
    assert!(
        harness.epochs().catalog > 0,
        "catalog epoch moves when tools are disabled"
    );
}

// ---------------------------------------------------------------------------------------------
// T07/T08 invocation and continuation
// ---------------------------------------------------------------------------------------------

async fn describe_and_invoke(
    harness: &Harness,
    arguments: Value,
) -> crate::tool_selection::service::InvokeResponse {
    let context = harness.context();
    harness.service.set_scenario(&context, scenario());
    let search = harness
        .service
        .search(&context, "search notes", 1)
        .await
        .expect("search");
    assert!(!search.candidates.is_empty(), "candidate expected");
    let describe = harness
        .service
        .describe(&context, &search.candidates[0].reference, "contract", None)
        .expect("describe");
    let execution_ref = describe.execution_ref.expect("execution ref");
    let cancellation = crate::RunCancellation::default();
    harness
        .service
        .invoke(&context, &execution_ref, &arguments, &cancellation)
        .await
        .expect("invoke")
}

#[tokio::test]
async fn t07_real_http_invoke_returns_success_and_records_one_call() {
    let state = default_state();
    let server = MockServer::start(state.clone()).await;
    let harness = Harness::new(&server, vec![user_grant("search")]);
    harness.manager.sync_source("mcp-test").await.expect("sync");
    let response = describe_and_invoke(&harness, json!({ "q": "value" })).await;
    assert_eq!(
        response.status,
        crate::tool_selection::backends::TechnicalStatus::Succeeded
    );
    assert_eq!(state.call_count.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn t07_is_error_result_is_failed_and_bounded() {
    let state = default_state();
    *state.call_result.lock().unwrap() = json!({
        "content": [{ "type": "text", "text": "boom" }],
        "isError": true
    });
    let server = MockServer::start(state).await;
    let harness = Harness::new(&server, vec![user_grant("search")]);
    harness.manager.sync_source("mcp-test").await.expect("sync");
    let response = describe_and_invoke(&harness, json!({ "q": "value" })).await;
    assert_eq!(
        response.status,
        crate::tool_selection::backends::TechnicalStatus::Failed
    );
    assert_eq!(response.error_code, Some("remote-tool-error"));
    assert!(response.result.is_some());
}

#[tokio::test]
async fn t07_disconnect_after_side_effect_is_unknown_and_not_retried() {
    let state = default_state();
    state.disconnect_after_call.store(true, Ordering::SeqCst);
    let server = MockServer::start(state.clone()).await;
    let harness = Harness::new(&server, vec![user_grant("search")]);
    harness.manager.sync_source("mcp-test").await.expect("sync");
    let response = describe_and_invoke(&harness, json!({ "q": "value" })).await;
    assert_eq!(
        response.status,
        crate::tool_selection::backends::TechnicalStatus::Unknown
    );
    // The server observed exactly one side effect and the client never retried.
    assert_eq!(state.call_count.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn t07_cancel_before_send_makes_zero_calls() {
    let state = default_state();
    let server = MockServer::start(state.clone()).await;
    let harness = Harness::new(&server, vec![user_grant("search")]);
    harness.manager.sync_source("mcp-test").await.expect("sync");
    let context = harness.context();
    harness.service.set_scenario(&context, scenario());
    let search = harness
        .service
        .search(&context, "search notes", 1)
        .await
        .expect("search");
    let describe = harness
        .service
        .describe(&context, &search.candidates[0].reference, "contract", None)
        .expect("describe");
    let execution_ref = describe.execution_ref.expect("execution ref");
    let cancellation = crate::RunCancellation::default();
    cancellation.cancel();
    let result = harness
        .service
        .invoke(
            &context,
            &execution_ref,
            &json!({ "q": "value" }),
            &cancellation,
        )
        .await;
    assert!(
        result.is_err(),
        "a cancelled-before-send call is reported to the caller"
    );
    assert_eq!(state.call_count.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn t08_large_result_is_paged_and_reassembles_exactly() {
    let state = default_state();
    let payload: String = "あ".repeat(40 * 1024); // 120 KiB of UTF-8
    *state.call_result.lock().unwrap() = json!({
        "content": [{ "type": "text", "text": payload }],
        "isError": false,
    });
    let server = MockServer::start(state).await;
    let harness = Harness::new(&server, vec![user_grant("search")]);
    harness.manager.sync_source("mcp-test").await.expect("sync");
    let response = describe_and_invoke(&harness, json!({ "q": "value" })).await;
    assert_eq!(
        response.status,
        crate::tool_selection::backends::TechnicalStatus::Succeeded
    );
    assert_eq!(response.result, None);
    assert_eq!(response.result_availability, Some("stored"));
    let result_ref = response.result_ref.clone().expect("result ref");
    let page_count = response.page_count.expect("page count");
    assert!(page_count > 1);

    let context = harness.context();
    let mut reassembled = String::new();
    for page in 0..page_count {
        let page = harness
            .service
            .describe_result(&context, &result_ref, page)
            .expect("page");
        reassembled.push_str(&page.text);
    }
    let expected = descriptors::canonical_json_string(&json!({
        "content": [{ "type": "text", "text": payload }],
        "isError": false,
    }));
    assert_eq!(reassembled, expected);
}

#[tokio::test]
async fn t08_result_scope_ttl_and_acl_are_enforced() {
    let state = default_state();
    *state.call_result.lock().unwrap() = json!({
        "content": [{ "type": "text", "text": "x".repeat(40 * 1024) }],
    });
    let server = MockServer::start(state).await;
    let harness = Harness::new(&server, vec![user_grant("search")]);
    harness.manager.sync_source("mcp-test").await.expect("sync");
    let response = describe_and_invoke(&harness, json!({ "q": "value" })).await;
    let result_ref = response.result_ref.expect("result ref");
    let context = harness.context();

    // Another run cannot read the stored result.
    let principal = harness.principal.clone();
    let other =
        RequestContext::new(&principal, "conversation-d4").with_run(Some("run-other".into()));
    assert!(harness
        .service
        .describe_result(&other, &result_ref, 0)
        .is_err());

    // An expired row is refused.
    harness
        .writer
        .write({
            let result_ref = result_ref.clone();
            move |connection| {
                connection
                    .execute(
                        "UPDATE tool_selection_mcp_results SET expires_at = 1 WHERE id = ?1",
                        rusqlite::params![result_ref],
                    )
                    .map_err(|e| e.to_string())?;
                Ok(())
            }
        })
        .unwrap();
    assert!(harness
        .service
        .describe_result(&context, &result_ref, 0)
        .is_err());
}

#[tokio::test]
async fn t08_size_limit_keeps_the_scan_bounded() {
    let state = default_state();
    *state.call_result.lock().unwrap() = json!({
        "content": [{ "type": "text", "text": "y".repeat(40 * 1024) }],
    });
    let server = MockServer::start(state).await;
    let harness = Harness::new(&server, vec![user_grant("search")]);
    harness.manager.sync_source("mcp-test").await.expect("sync");
    let tool_id = descriptors::tool_id("mcp-test", "search");
    let revision_id = harness
        .writer
        .read_serialized({
            let tool_id = tool_id.clone();
            move |c| {
                repository::tool_by_id(c, &tool_id)
                    .map_err(|e| e.to_string())?
                    .and_then(|tool| tool.current_revision_id)
                    .ok_or_else(|| "missing".to_string())
            }
        })
        .unwrap();
    // A payload above 1 MiB is reported as a size limit and is never inserted.
    let oversized = format!(
        "{{\"big\":\"{}\"}}",
        "z".repeat(super::MCP_RESULT_MAX_BYTES)
    );
    let outcome = harness
        .writer
        .write(move |connection| {
            let tx = connection
                .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
                .map_err(crate::database_error)?;
            let outcome = results::store_result(
                &tx,
                "missing-invocation",
                PRINCIPAL,
                "conversation-d4",
                "run-d4",
                &tool_id,
                &revision_id,
                &"a".repeat(64),
                0,
                &oversized,
            )
            .map_err(|error| error.code.as_str().to_string())?;
            tx.commit().map_err(crate::database_error)?;
            Ok(outcome)
        })
        .unwrap();
    assert_eq!(outcome, results::StoreOutcome::SizeLimit);
}

// ---------------------------------------------------------------------------------------------
// T09 freshness
// ---------------------------------------------------------------------------------------------

#[tokio::test]
async fn t09_stale_source_is_not_eligible() {
    let state = default_state();
    let server = MockServer::start(state.clone()).await;
    let harness = Harness::new(&server, vec![user_grant("search")]);
    harness.manager.sync_source("mcp-test").await.expect("sync");
    // Age the last success beyond the freshness window.
    harness
        .writer
        .write(|connection| {
            connection
                .execute(
                    "UPDATE tool_selection_mcp_sources SET last_success_at = ?1 WHERE source_id = 'mcp-test'",
                    rusqlite::params![now_ms() - super::MCP_SOURCE_STALE_AFTER_MILLIS - 1000],
                )
                .map_err(|e| e.to_string())?;
            Ok(())
        })
        .unwrap();
    let eligible = harness
        .writer
        .read_serialized({
            let principal = harness.principal.clone();
            move |c| {
                repository::eligible_revisions(c, &principal, None, now_ms())
                    .map(|rows| rows.len())
                    .map_err(|e| e.to_string())
            }
        })
        .unwrap();
    assert_eq!(eligible, 0);

    // A remote invoke is refused before any HTTP call.
    let before = state.call_count.load(Ordering::SeqCst);
    let context = harness.context();
    harness.service.set_scenario(&context, scenario());
    // Search returns no candidate because the source is stale, so there is nothing to describe.
    let search = harness
        .service
        .search(&context, "search notes", 1)
        .await
        .expect("search");
    assert!(search.candidates.is_empty());
    assert_eq!(state.call_count.load(Ordering::SeqCst), before);
}

// ---------------------------------------------------------------------------------------------
// T10 same-name ambiguity
// ---------------------------------------------------------------------------------------------

#[tokio::test]
async fn t10_same_name_in_two_sources_makes_a_name_only_correction_ambiguous() {
    // Two MCP sources both publish `search`. A name-only correction must not pick one.
    let state_a = default_state();
    let state_b = default_state();
    let server_a = MockServer::start(state_a).await;
    let server_b = MockServer::start(state_b).await;
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
                .map_err(|e| e.to_string())?;
            Ok(())
        })
        .unwrap();
    let principal = crate::tool_selection::service::ensure_principal(&writer).expect("principal");
    let sources = McpSources {
        sources: vec![
            McpSourceSpec {
                id: "mcp-a".to_string(),
                url: server_a.url(),
                enabled: true,
                bearer_token_env: None,
                grants: vec![user_grant("search")],
            },
            McpSourceSpec {
                id: "mcp-b".to_string(),
                url: server_b.url(),
                enabled: true,
                bearer_token_env: None,
                grants: vec![user_grant("search")],
            },
        ],
    };
    let manager = McpManager::new(
        writer.clone(),
        principal.clone(),
        None,
        sources,
        Some(hash_embedding()),
        None,
    );
    manager.sync_source("mcp-a").await.expect("a");
    manager.sync_source("mcp-b").await.expect("b");
    let id_a = descriptors::tool_id("mcp-a", "search");
    let id_b = descriptors::tool_id("mcp-b", "search");
    assert_ne!(id_a, id_b);

    // Both are authorized, so a name-only target is ambiguous and produces no rule.
    let context = RequestContext::new(&principal, "conversation-d4")
        .with_run(Some("run-d4".into()))
        .with_message(Some("msg-d4".into()));
    writer
        .write(|connection| {
            connection
                .execute(
                    "INSERT INTO conversation_messages(id, conversation_id, role, content, created_at)
                     VALUES ('msg-d4', 'conversation-d4', 'user', 'fixture', '1')",
                    [],
                )
                .map_err(|e| e.to_string())?;
            Ok(())
        })
        .unwrap();
    let parsed = crate::tool_selection::feedback::ParsedExtraction {
        scenario: scenario(),
        accepted: vec![crate::tool_selection::contracts::ExtractedFeedback {
            kind: crate::tool_selection::contracts::FeedbackKind::ToolChoice,
            decision_id: None,
            rejected_tool_id: Some("search".to_string()),
            preferred_tool_id: None,
            scope: crate::tool_selection::contracts::ScopeKind::Project,
            duration: crate::tool_selection::contracts::Duration::Persistent,
            evidence: crate::tool_selection::contracts::Evidence {
                start: 0,
                end: 6,
                text: "search".to_string(),
            },
            condition: crate::tool_selection::contracts::FeedbackCondition {
                operation: Some(crate::tool_selection::Operation::Search),
                object_type: Some(crate::tool_selection::ObjectType::Document),
                phase: None,
                input_kind: None,
            },
        }],
        rejected: Vec::new(),
    };
    let mut service = ToolSelectionService::new(
        writer.clone(),
        hash_embedding(),
        Arc::new(FixedReranker::new(&[])),
        Arc::new(UnconfiguredExtractor),
        Arc::new(FixtureBackend::new()),
        f64::NEG_INFINITY,
    );
    service.set_mcp_manager(manager);
    let outcome = service.apply_parsed(&context, &parsed).expect("apply");
    assert!(outcome.ambiguous);
    let rules: i64 = writer
        .read_serialized(|c| {
            c.query_row("SELECT COUNT(*) FROM tool_selection_rules", [], |row| {
                row.get(0)
            })
            .map_err(|e| e.to_string())
        })
        .unwrap();
    assert_eq!(rules, 0);
}

// ---------------------------------------------------------------------------------------------
// A-series: scale and gateway shape
// ---------------------------------------------------------------------------------------------

#[tokio::test]
async fn a01_two_sources_with_1500_tools_publish_and_search_is_bounded() {
    let state_a = Arc::new(ServerState {
        protocol_version: Mutex::new(super::MCP_PROTOCOL_VERSION.to_string()),
        tools_capability: AtomicBool::new(true),
        ..ServerState::default()
    });
    let state_b = Arc::new(ServerState {
        protocol_version: Mutex::new(super::MCP_PROTOCOL_VERSION.to_string()),
        tools_capability: AtomicBool::new(true),
        ..ServerState::default()
    });
    let page = |prefix: &str, start: usize, count: usize| {
        let tools: Vec<Value> = (start..start + count)
            .map(|index| {
                json!({
                    "name": format!("{prefix}_{index:04}"),
                    "description": format!("Tool {index} for {prefix}."),
                    "inputSchema": { "type": "object", "properties": { "q": { "type": "string" } } }
                })
            })
            .collect();
        json!({ "tools": tools })
    };
    state_a.list_pages.lock().unwrap().clear();
    for chunk in 0..5 {
        state_a
            .list_pages
            .lock()
            .unwrap()
            .push(page("alpha", chunk * 100, 100));
    }
    state_b.list_pages.lock().unwrap().clear();
    for chunk in 0..10 {
        state_b
            .list_pages
            .lock()
            .unwrap()
            .push(page("beta", chunk * 100, 100));
    }
    let server_a = MockServer::start(state_a).await;
    let server_b = MockServer::start(state_b).await;

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
                .map_err(|e| e.to_string())?;
            Ok(())
        })
        .unwrap();
    let principal = crate::tool_selection::service::ensure_principal(&writer).expect("principal");
    let grants: Vec<McpGrantSpec> = Vec::new();
    let sources = McpSources {
        sources: vec![
            McpSourceSpec {
                id: "alpha".to_string(),
                url: server_a.url(),
                enabled: true,
                bearer_token_env: None,
                grants: grants.clone(),
            },
            McpSourceSpec {
                id: "beta".to_string(),
                url: server_b.url(),
                enabled: true,
                bearer_token_env: None,
                grants,
            },
        ],
    };
    // Grant every tool through a wildcard-free loop after sync, matching how the config grants
    // are declarative per tool. For scale we insert user grants directly.
    let manager = McpManager::new(
        writer.clone(),
        principal.clone(),
        None,
        sources,
        Some(hash_embedding()),
        None,
    );
    manager.sync_source("alpha").await.expect("alpha");
    manager.sync_source("beta").await.expect("beta");
    let tool_count = writer
        .read_serialized(|c| mcp_repo::published_tool_count(c, "alpha").map_err(|e| e.to_string()))
        .unwrap();
    assert_eq!(tool_count, 500);
    writer
        .write({
            let principal = principal.clone();
            move |connection| {
                let tx = connection
                    .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
                    .map_err(crate::database_error)?;
                tx.execute(
                    "INSERT OR IGNORE INTO tool_selection_grants(principal_id, tool_id, scope_kind, scope_id)
                     SELECT ?1, id, 'user', ?1 FROM tool_selection_catalog",
                    rusqlite::params![principal],
                )
                .map_err(|e| e.to_string())?;
                repository::bump_epochs(&tx, false, true, false).map_err(|e| e.to_string())?;
                tx.commit().map_err(crate::database_error)?;
                Ok(())
            }
        })
        .unwrap();

    let mut service = ToolSelectionService::new(
        writer.clone(),
        hash_embedding(),
        Arc::new(FixedReranker::new(&[])),
        Arc::new(UnconfiguredExtractor),
        Arc::new(FixtureBackend::new()),
        f64::NEG_INFINITY,
    );
    service.set_discovery_configured(true);
    service.set_mcp_manager(manager);
    let context =
        RequestContext::new(&principal, "conversation-d4").with_run(Some("run-d4".into()));
    service.set_scenario(&context, scenario());
    let response = service
        .search(&context, "alpha 0001", 8)
        .await
        .expect("search");
    assert!(response.candidates.len() <= 8);
    // The three conversation tools are still exactly three definitions.
    let definitions = crate::tool_selection::gateway::definitions();
    assert_eq!(definitions.len(), 3);
}

// Alias so the tests can call the MCP table accessors without a long path.
use super::repository as mcp_repo;

// ---------------------------------------------------------------------------------------------
// T02 migration
// ---------------------------------------------------------------------------------------------

#[test]
fn t02_v23_sources_are_rebuilt_without_losing_data() {
    let directory = tempfile::tempdir().expect("tempdir");
    let path = directory.path().join("ledger.sqlite");
    let connection = Connection::open(&path).expect("open");
    connection
        .execute_batch(
            "PRAGMA foreign_keys = ON;
             CREATE TABLE tool_selection_sources (
               id TEXT PRIMARY KEY,
               kind TEXT NOT NULL CHECK(kind IN ('llang')),
               owner_principal TEXT NOT NULL CHECK(length(owner_principal) BETWEEN 1 AND 160),
               enabled INTEGER NOT NULL CHECK(enabled IN (0, 1))
             );
             CREATE TABLE tool_selection_catalog (
               id TEXT PRIMARY KEY,
               source_id TEXT NOT NULL,
               backend_key TEXT NOT NULL,
               current_revision_id TEXT,
               enabled INTEGER NOT NULL,
               FOREIGN KEY(source_id) REFERENCES tool_selection_sources(id)
             );
             INSERT INTO tool_selection_sources(id, kind, owner_principal, enabled)
               VALUES ('llang', 'llang', 'P1', 1);
             INSERT INTO tool_selection_catalog(id, source_id, backend_key, current_revision_id, enabled)
               VALUES ('legacy-tool', 'llang', 'web', NULL, 1);",
        )
        .expect("old schema");
    // Running the current migration widens the CHECK constraint and preserves rows.
    crate::persistence::schema::initialize_database(&connection).expect("migrate");
    let kind: String = connection
        .query_row(
            "SELECT kind FROM tool_selection_sources WHERE id = 'llang'",
            [],
            |row| row.get(0),
        )
        .expect("preserved source");
    assert_eq!(kind, "llang");
    let catalog: i64 = connection
        .query_row("SELECT COUNT(*) FROM tool_selection_catalog", [], |row| {
            row.get(0)
        })
        .expect("catalog");
    assert_eq!(catalog, 1);
    // The new kind is accepted by the rebuilt CHECK constraint.
    connection
        .execute(
            "INSERT INTO tool_selection_sources(id, kind, owner_principal, enabled)
             VALUES ('mcp', 'mcp_http', 'P1', 1)",
            [],
        )
        .expect("mcp_http accepted");
    let violations: i64 = connection
        .query_row("SELECT COUNT(*) FROM pragma_foreign_key_check", [], |row| {
            row.get(0)
        })
        .expect("fk check");
    assert_eq!(violations, 0);
    drop(connection);
    // The database reopens with the same schema version.
    let reopened = Connection::open(&path).expect("reopen");
    let version: i64 = reopened
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .expect("version");
    assert_eq!(version, crate::persistence::schema::DATABASE_SCHEMA_VERSION);
}

// ---------------------------------------------------------------------------------------------
// T06 pending grants appear after a later sync
// ---------------------------------------------------------------------------------------------

#[tokio::test]
async fn t06_grant_declared_before_the_tool_appears_is_applied_on_a_later_sync() {
    let state = default_state();
    let server = MockServer::start(state.clone()).await;
    let harness = Harness::new(&server, vec![user_grant("later")]);
    harness
        .manager
        .sync_source("mcp-test")
        .await
        .expect("first");
    let later_id = descriptors::tool_id("mcp-test", "later");
    let principal = harness.principal.clone();
    let before = harness
        .writer
        .read_serialized({
            let principal = principal.clone();
            let later_id = later_id.clone();
            move |c| {
                repository::grant_exists(c, &principal, &later_id, None).map_err(|e| e.to_string())
            }
        })
        .unwrap();
    assert!(!before, "a declaration for a missing tool stays pending");

    state.list_pages.lock().unwrap()[0]["tools"] = json!([
        { "name": "later", "inputSchema": { "type": "object" } }
    ]);
    harness
        .manager
        .sync_source("mcp-test")
        .await
        .expect("second");
    let after = harness
        .writer
        .read_serialized(move |c| {
            repository::grant_exists(c, &principal, &later_id, None).map_err(|e| e.to_string())
        })
        .unwrap();
    assert!(
        after,
        "the pending declaration is applied once the name appears"
    );
}

// ---------------------------------------------------------------------------------------------
// T07 binding validation refuses before send
// ---------------------------------------------------------------------------------------------

#[tokio::test]
async fn t07_endpoint_change_and_unknown_kind_are_refused_before_send() {
    let state = default_state();
    let server = MockServer::start(state.clone()).await;
    let harness = Harness::new(&server, vec![user_grant("search")]);
    harness.manager.sync_source("mcp-test").await.expect("sync");
    let backend = McpBackend::new(harness.manager.clone());
    let cancellation = crate::RunCancellation::default();

    // A changed endpoint hash is rejected before any HTTP call.
    let tampered = crate::tool_selection::backends::BackendRequest {
        call_id: "c1".to_string(),
        tool_id: descriptors::tool_id("mcp-test", "search"),
        revision_id: "r1".to_string(),
        backend_key: "search".to_string(),
        binding: json!({
            "kind": "mcp_http",
            "sourceId": "mcp-test",
            "toolName": "search",
            "endpointHash": "f".repeat(64),
        }),
        arguments: json!({ "q": "v" }),
        timeout: std::time::Duration::from_secs(5),
    };
    let outcome =
        crate::tool_selection::backends::ToolBackend::invoke(&backend, tampered, &cancellation)
            .await;
    assert_eq!(
        outcome.status,
        crate::tool_selection::backends::TechnicalStatus::Failed
    );
    assert_eq!(state.call_count.load(Ordering::SeqCst), 0);

    // A missing kind never falls back to the remote backend. A complete legacy L-Lang binding is
    // recognized; anything else is refused.
    let llang_binding = json!({
        "capabilityId": "x",
        "revisionId": "r",
        "packageHash": "p",
        "contractHash": "c",
        "catalogEpoch": 0,
        "inputFields": []
    });
    assert_eq!(BackendRouter::kind(&llang_binding), "llang");
    assert_eq!(
        BackendRouter::kind(&json!({ "capabilityId": "x" })),
        "unknown"
    );
    assert_eq!(BackendRouter::kind(&json!({ "kind": "other" })), "unknown");
}

// ---------------------------------------------------------------------------------------------
// T09 degraded embedding lane
// ---------------------------------------------------------------------------------------------

#[tokio::test]
async fn t09_missing_embeddings_degrade_to_lexical_without_inventing_confidence() {
    let state = default_state();
    let server = MockServer::start(state).await;
    // A manager with no embedding provider leaves the vector lane empty.
    let harness = Harness::new_with_embedder(&server, vec![user_grant("search")], false);
    harness.manager.sync_source("mcp-test").await.expect("sync");
    let context = harness.context();
    harness.service.set_scenario(&context, scenario());
    let response = harness
        .service
        .search(&context, "search notes", 3)
        .await
        .expect("search");
    assert_eq!(
        response.status,
        crate::tool_selection::contracts::DecisionStatus::Degraded
    );
    assert!(response.degraded);
}

// ---------------------------------------------------------------------------------------------
// A06/A15 transport edge cases
// ---------------------------------------------------------------------------------------------

#[tokio::test]
async fn a06_progress_notifications_are_ignored_and_the_result_is_returned() {
    let state = default_state();
    state.sse.store(true, Ordering::SeqCst);
    state.emit_progress.store(true, Ordering::SeqCst);
    let server = MockServer::start(state).await;
    let harness = Harness::new(&server, vec![user_grant("search")]);
    harness.manager.sync_source("mcp-test").await.expect("sync");
    let response = describe_and_invoke(&harness, json!({ "q": "v" })).await;
    assert_eq!(
        response.status,
        crate::tool_selection::backends::TechnicalStatus::Succeeded
    );
}

#[tokio::test]
async fn a15_unauthorized_is_a_remote_failure() {
    let state = default_state();
    state.unauthorized.store(true, Ordering::SeqCst);
    let server = MockServer::start(state).await;
    let harness = Harness::new(&server, vec![user_grant("search")]);
    let error = harness
        .manager
        .sync_source("mcp-test")
        .await
        .expect_err("401 fails the sync");
    assert_eq!(error.code, "remote-unauthorized");
    // No source success is recorded for an unauthorized server.
    let fresh = harness
        .writer
        .read_serialized(|c| mcp_repo::is_fresh(c, "mcp-test", now_ms()).map_err(|e| e.to_string()))
        .unwrap();
    assert!(!fresh);
}

#[tokio::test]
async fn a15_unsupported_server_request_is_refused_with_method_not_found() {
    let state = default_state();
    state.server_request.store(true, Ordering::SeqCst);
    state.get_supported.store(true, Ordering::SeqCst);
    let server = MockServer::start(state.clone()).await;
    let harness = Harness::new(&server, vec![user_grant("search")]);
    harness.manager.sync_source("mcp-test").await.expect("sync");
    // The GET watcher reads the server request and replies with -32601; the request is never run.
    let mut reply = None;
    for _ in 0..100 {
        reply = *state.unsupported_reply.lock().unwrap();
        if reply.is_some() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert_eq!(reply, Some(-32601));
}

#[tokio::test]
async fn t07_restart_reconcile_marks_mcp_calls_as_indeterminate() {
    let state = default_state();
    let server = MockServer::start(state).await;
    let harness = Harness::new(&server, vec![user_grant("search")]);
    harness.manager.sync_source("mcp-test").await.expect("sync");
    let tool_id = descriptors::tool_id("mcp-test", "search");
    let revision_id = harness
        .writer
        .read_serialized(move |c| {
            repository::tool_by_id(c, &tool_id)
                .map_err(|e| e.to_string())?
                .and_then(|tool| tool.current_revision_id)
                .ok_or_else(|| "missing".to_string())
        })
        .unwrap();
    harness
        .writer
        .write({
            let revision_id = revision_id.clone();
            move |connection| {
                connection
                    .execute(
                        "INSERT INTO tool_selection_invocations(
                           id, decision_id, revision_id, technical_status, satisfaction, started_at)
                         VALUES ('mcp-running', NULL, ?1, 'running', 'unknown', 1)",
                        rusqlite::params![revision_id],
                    )
                    .map_err(|e| e.to_string())?;
                Ok(())
            }
        })
        .unwrap();
    crate::tool_selection::service::reconcile_interrupted_invocations(&harness.writer)
        .expect("reconcile");
    let (status, error): (String, Option<String>) = harness
        .writer
        .read_serialized(|c| {
            c.query_row(
                "SELECT technical_status, error_code FROM tool_selection_invocations WHERE id = 'mcp-running'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .map_err(|e| e.to_string())
        })
        .unwrap();
    assert_eq!(status, "interrupted");
    assert_eq!(error.as_deref(), Some("remote-outcome-unknown"));
}

// ---------------------------------------------------------------------------------------------
// Review fixes: resolution, backoff, admission, removal race, manual grants, project ACL
// ---------------------------------------------------------------------------------------------

#[test]
fn review_source_qualified_resolution_is_unambiguous() {
    let connection = Connection::open_in_memory().expect("in-memory");
    crate::persistence::schema::initialize_database(&connection).expect("schema");
    let principal = "P-REVIEW";
    let a = descriptors::tool_id("mcp-a", "search");
    let b = descriptors::tool_id("mcp-b", "search");
    for (source, tool) in [("mcp-a", &a), ("mcp-b", &b)] {
        repository::upsert_source(&connection, source, "mcp_http", principal, true)
            .expect("source");
        repository::upsert_tool(
            &connection,
            &repository::NewTool {
                source_id: source,
                tool_id: tool,
                backend_key: "search",
                enabled: true,
            },
        )
        .expect("tool");
        repository::upsert_grant(&connection, principal, tool, "user", principal).expect("grant");
    }
    let context = RequestContext::new(principal, "conversation-d4").with_run(Some("run".into()));
    // A bare duplicate name is ambiguous.
    assert_eq!(
        crate::tool_selection::resolve::resolve_tool_id(&connection, "search", &context, None),
        None
    );
    // A source-qualified name resolves to exactly one tool.
    assert_eq!(
        crate::tool_selection::resolve::resolve_tool_id(
            &connection,
            "mcp-a/search",
            &context,
            None
        ),
        Some(a)
    );
    assert_eq!(
        crate::tool_selection::resolve::resolve_tool_id(
            &connection,
            "mcp-b/search",
            &context,
            None
        ),
        Some(b)
    );
}

#[tokio::test]
async fn review_failed_initialize_sets_a_source_backoff() {
    let state = default_state();
    state.tools_capability.store(false, Ordering::SeqCst);
    let server = MockServer::start(state).await;
    let harness = Harness::new(&server, vec![user_grant("search")]);
    let first = harness.manager.sync_source("mcp-test").await.unwrap_err();
    assert_eq!(first.code, "tools-capability-missing");
    // The immediate retry is refused by the 1s backoff rather than hammering the server.
    let second = harness.manager.sync_source("mcp-test").await.unwrap_err();
    assert_eq!(second.code, "source-backoff");
}

#[tokio::test]
async fn review_removal_during_first_sync_cannot_republish_the_source() {
    let state = default_state();
    state.gate_list.store(true, Ordering::SeqCst);
    let server = MockServer::start(state.clone()).await;
    let harness = Harness::new(&server, vec![user_grant("search")]);
    let manager = harness.manager.clone();
    let sync = tokio::spawn(async move { manager.sync_source("mcp-test").await });
    // Barrier: the server has received tools/list but has not answered it yet.
    state.list_received.notified().await;
    harness
        .manager
        .apply_sources(McpSources {
            sources: Vec::new(),
        })
        .await;
    state.list_release.notify_one();
    let outcome = sync.await.expect("join");
    assert_eq!(outcome.unwrap_err().code, "sync-generation-changed");
    let enabled = harness
        .writer
        .read_serialized(|c| {
            c.query_row(
                "SELECT enabled FROM tool_selection_sources WHERE id = 'mcp-test'",
                [],
                |row| row.get::<_, i64>(0),
            )
            .map_err(|e| e.to_string())
        })
        .unwrap();
    assert_eq!(enabled, 0, "a removed source stays disabled");
    let tools = harness
        .writer
        .read_serialized(|c| {
            mcp_repo::published_tool_count(c, "mcp-test").map_err(|e| e.to_string())
        })
        .unwrap();
    assert_eq!(tools, 0, "no tool was published by the aborted sync");
}

#[tokio::test]
async fn review_manual_grant_is_never_adopted_or_revoked() {
    let state = default_state();
    let server = MockServer::start(state).await;
    let harness = Harness::new(&server, Vec::new());
    harness.manager.sync_source("mcp-test").await.expect("sync");
    let tool_id = descriptors::tool_id("mcp-test", "search");
    let principal = harness.principal.clone();
    harness
        .writer
        .write({
            let tool_id = tool_id.clone();
            let principal = principal.clone();
            move |c| {
                repository::upsert_grant(c, &principal, &tool_id, "user", &principal)
                    .map_err(|e| e.to_string())?;
                Ok(())
            }
        })
        .unwrap();
    // Now the config declares the same grant and a sync runs.
    harness
        .manager
        .apply_sources(McpSources {
            sources: vec![McpSourceSpec {
                id: "mcp-test".to_string(),
                url: server.url(),
                enabled: true,
                bearer_token_env: None,
                grants: vec![user_grant("search")],
            }],
        })
        .await;
    harness
        .manager
        .sync_source("mcp-test")
        .await
        .expect("sync2");
    let managed = harness
        .writer
        .read_serialized(|c| {
            c.query_row(
                "SELECT COUNT(*) FROM tool_selection_mcp_managed_grants",
                [],
                |row| row.get::<_, i64>(0),
            )
            .map_err(|e| e.to_string())
        })
        .unwrap();
    assert_eq!(managed, 0, "a pre-existing manual grant is not adopted");
    // Removing the source must leave the manual grant intact.
    harness
        .manager
        .apply_sources(McpSources {
            sources: Vec::new(),
        })
        .await;
    let still_granted = harness
        .writer
        .read_serialized({
            let tool_id = tool_id.clone();
            let principal = principal.clone();
            move |c| {
                repository::exact_grant_exists(c, &principal, &tool_id, "user", &principal)
                    .map_err(|e| e.to_string())
            }
        })
        .unwrap();
    assert!(still_granted, "the manual grant survives source removal");
}

#[tokio::test]
async fn review_after_send_cancel_is_unknown_and_releases_permits() {
    let state = default_state();
    state.gate_call.store(true, Ordering::SeqCst);
    let server = MockServer::start(state.clone()).await;
    let harness = Harness::new(&server, vec![user_grant("search")]);
    harness.manager.sync_source("mcp-test").await.expect("sync");
    let context = harness.context();
    harness.service.set_scenario(&context, scenario());
    let search = harness
        .service
        .search(&context, "search notes", 1)
        .await
        .expect("search");
    let describe = harness
        .service
        .describe(&context, &search.candidates[0].reference, "contract", None)
        .expect("describe");
    let execution_ref = describe.execution_ref.expect("execution ref");
    let cancellation = crate::RunCancellation::default();
    let arguments = json!({ "q": "v" });
    let invoke = harness
        .service
        .invoke(&context, &execution_ref, &arguments, &cancellation);
    let controller = async {
        state.call_received.notified().await;
        cancellation.cancel();
        state.gate_call.store(false, Ordering::SeqCst);
        state.call_release.notify_one();
    };
    let (result, _) = tokio::join!(invoke, controller);
    let response = result.expect("invoke");
    assert_eq!(
        response.status,
        crate::tool_selection::backends::TechnicalStatus::Unknown
    );
    // The permit was released, so a second call is admitted and succeeds.
    let second = harness
        .service
        .invoke(
            &context,
            &execution_ref,
            &json!({ "q": "v" }),
            &crate::RunCancellation::default(),
        )
        .await
        .expect("second invoke");
    assert_eq!(
        second.status,
        crate::tool_selection::backends::TechnicalStatus::Succeeded
    );
    assert_eq!(state.call_count.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn review_project_scoped_result_is_refused_for_another_project() {
    let state = default_state();
    *state.call_result.lock().unwrap() = json!({
        "content": [{ "type": "text", "text": "x".repeat(40 * 1024) }],
    });
    let server = MockServer::start(state).await;
    let harness = Harness::new(&server, Vec::new());
    harness.manager.sync_source("mcp-test").await.expect("sync");
    let tool_id = descriptors::tool_id("mcp-test", "search");
    let principal = harness.principal.clone();
    harness
        .writer
        .write({
            let tool_id = tool_id.clone();
            let principal = principal.clone();
            move |c| {
                repository::upsert_grant(c, &principal, &tool_id, "project", "PROJ-1")
                    .map_err(|e| e.to_string())?;
                repository::bump_epochs(c, false, true, false).map_err(|e| e.to_string())?;
                Ok(())
            }
        })
        .unwrap();
    let context = RequestContext::new(&principal, "conversation-d4")
        .with_run(Some("run-proj".into()))
        .with_project(Some("PROJ-1".into()));
    harness.service.set_scenario(&context, scenario());
    let search = harness
        .service
        .search(&context, "search notes", 1)
        .await
        .expect("search");
    assert_eq!(search.candidates.len(), 1);
    let describe = harness
        .service
        .describe(&context, &search.candidates[0].reference, "contract", None)
        .expect("describe");
    let execution_ref = describe.execution_ref.expect("execution ref");
    let response = harness
        .service
        .invoke(
            &context,
            &execution_ref,
            &json!({ "q": "v" }),
            &crate::RunCancellation::default(),
        )
        .await
        .expect("invoke");
    let result_ref = response.result_ref.expect("result ref");
    assert!(harness
        .service
        .describe_result(&context, &result_ref, 0)
        .is_ok());
    // The same run scope but a different project must not read the stored result.
    let other = RequestContext::new(&principal, "conversation-d4")
        .with_run(Some("run-proj".into()))
        .with_project(Some("PROJ-2".into()));
    assert!(harness
        .service
        .describe_result(&other, &result_ref, 0)
        .is_err());
}

#[tokio::test]
async fn review_describe_refuses_a_stale_source_reference() {
    let state = default_state();
    let server = MockServer::start(state).await;
    let harness = Harness::new(&server, vec![user_grant("search")]);
    harness.manager.sync_source("mcp-test").await.expect("sync");
    let context = harness.context();
    harness.service.set_scenario(&context, scenario());
    let search = harness
        .service
        .search(&context, "search notes", 1)
        .await
        .expect("search");
    let candidate_ref = search.candidates[0].reference.clone();
    // Age the source beyond the freshness window after the reference was issued.
    harness
        .writer
        .write(|c| {
            c.execute(
                "UPDATE tool_selection_mcp_sources SET last_success_at = ?1 WHERE source_id = 'mcp-test'",
                rusqlite::params![now_ms() - super::MCP_SOURCE_STALE_AFTER_MILLIS - 1000],
            )
            .map_err(|e| e.to_string())?;
            Ok(())
        })
        .unwrap();
    let error = harness
        .service
        .describe(&context, &candidate_ref, "contract", None)
        .unwrap_err();
    assert_eq!(
        error.code,
        crate::tool_selection::ToolSelectionErrorCode::StaleReference
    );
}
