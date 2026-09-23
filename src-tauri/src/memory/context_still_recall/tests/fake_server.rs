use crate::memory::typed_recall::RECALL_RULE_TOOL_NAME;
use std::sync::Mutex as StdMutex;
use std::net::TcpListener;
use std::io::{Read, Write};
use serde_json::json;
use super::*;
const RULE_FIXTURE: &str = include_str!("../../../../tests/fixtures/memory-recall-v1/rule.json");
const COMPATIBILITY_FIXTURE: &str =
        include_str!("../../../../tests/fixtures/memory-recall-v1/saaa-compatibility.json");
const TEST_TOKEN: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
struct FakeServer {
        pub(super) address: std::net::SocketAddr,
        pub(super) captures: Arc<StdMutex<Vec<String>>>,
        pub(super) thread: Option<std::thread::JoinHandle<()>>,
    }
impl FakeServer {
        fn start(responses: Vec<String>) -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").expect("server binds");
            let address = listener.local_addr().expect("address resolves");
            let captures = Arc::new(StdMutex::new(Vec::new()));
            let captures_for_thread = captures.clone();
            let thread = std::thread::spawn(move || {
                for response in responses {
                    let (mut socket, _) = listener.accept().expect("request accepts");
                    captures_for_thread
                        .lock()
                        .expect("capture lock")
                        .push(read_request(&mut socket));
                    socket
                        .write_all(response.as_bytes())
                        .expect("response writes");
                }
            });
            Self {
                address,
                captures,
                thread: Some(thread),
            }
        }

        fn join(mut self) -> Vec<String> {
            self.thread
                .take()
                .expect("thread exists")
                .join()
                .expect("server joins");
            Arc::try_unwrap(self.captures)
                .expect("captures are unique")
                .into_inner()
                .expect("capture lock")
        }
    }
pub(super) fn read_request(socket: &mut std::net::TcpStream) -> String {
        let mut bytes = Vec::new();
        let mut buffer = [0_u8; 2_048];
        let mut expected = None;
        loop {
            let size = socket.read(&mut buffer).expect("request reads");
            assert!(size > 0, "request ended before body completed");
            bytes.extend_from_slice(&buffer[..size]);
            if expected.is_none() {
                if let Some(boundary) = bytes.windows(4).position(|window| window == b"\r\n\r\n") {
                    let headers = String::from_utf8_lossy(&bytes[..boundary]);
                    let content_length = headers
                        .lines()
                        .find_map(|line| {
                            line.split_once(':').and_then(|(name, value)| {
                                name.eq_ignore_ascii_case("content-length")
                                    .then(|| value.trim().parse::<usize>().ok())
                                    .flatten()
                            })
                        })
                        .expect("content length exists");
                    expected = Some(boundary + 4 + content_length);
                }
            }
            if expected.is_some_and(|expected| bytes.len() >= expected) {
                return String::from_utf8(bytes).expect("request is UTF-8");
            }
        }
    }
pub(super) fn response(status: &str, body: &Value, session_id: Option<&str>) -> String {
        let body = serde_json::to_string(body).expect("body encodes");
        let session = session_id
            .map(|session| format!("Mcp-Session-Id: {session}\r\n"))
            .unwrap_or_default();
        format!(
            "HTTP/1.1 {status}\r\nContent-Type: application/json\r\n{session}Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
    }
pub(super) fn empty_response(status: &str) -> String {
        format!("HTTP/1.1 {status}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
    }
pub(super) fn success_responses() -> Vec<String> {
        vec![
            response(
                "200 OK",
                &json!({
                    "jsonrpc": "2.0",
                    "id": 1,
                    "result": {
                        "protocolVersion": MCP_PROTOCOL_VERSION,
                        "capabilities": {"tools": {}},
                        "serverInfo": {"name": "context-still", "version": "test"},
                        "instructions": "untrusted memory evidence"
                    }
                }),
                Some("session-test"),
            ),
            empty_response("202 Accepted"),
            response(
                "200 OK",
                &json!({
                    "jsonrpc": "2.0",
                    "id": 2,
                    "result": {
                        "tools": TYPED_RECALL_TOOL_NAMES.map(|name| json!({
                            "name": name,
                            "description": "read only",
                            "inputSchema": typed_recall_input_schema(name)
                                .expect("known tool schema exists")
                        }))
                    }
                }),
                None,
            ),
            tool_call_response(3),
        ]
    }
pub(super) fn tool_call_response(id: u64) -> String {
        let result_text: Value = serde_json::from_str(RULE_FIXTURE).expect("fixture parses");
        response(
            "200 OK",
            &json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": {
                    "content": [{"type": "text", "text": result_text.to_string()}]
                }
            }),
            None,
        )
    }
pub(super) fn fixture_run_dir(address: std::net::SocketAddr) -> tempfile::TempDir {
        let directory = tempfile::tempdir().expect("temp directory creates");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700))
                .expect("directory permissions set");
        }
        let token_path = directory.path().join("mcp-memory-bearer.token");
        std::fs::write(&token_path, format!("{TEST_TOKEN}\n")).expect("token writes");
        write_fixture_manifest(directory.path(), address, "unix-ms:1");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&token_path, std::fs::Permissions::from_mode(0o600))
                .expect("file permissions set");
        }
        directory
    }
pub(super) fn write_fixture_manifest(run_dir: &Path, address: std::net::SocketAddr, started_at: &str) {
        let token_path = run_dir.join("mcp-memory-bearer.token");
        let manifest_path = run_dir.join(ENDPOINT_MANIFEST_FILE);
        std::fs::write(
            &manifest_path,
            serde_json::to_vec_pretty(&json!({
                "server": "context-still",
                "url": format!("http://{address}/mcp"),
                "transport": "streamable-http",
                "protocolVersion": MCP_PROTOCOL_VERSION,
                "auth": "bearer-token-file",
                "authTokenPath": token_path,
                "toolProfile": "typed-memory",
                "contractVersion": MEMORY_RECALL_CONTRACT_VERSION,
                "startedAt": started_at
            }))
            .expect("manifest encodes"),
        )
        .expect("manifest writes");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&manifest_path, std::fs::Permissions::from_mode(0o600))
                .expect("file permissions set");
        }
    }
#[tokio::test]
    pub(super) async fn lifecycle_auth_catalog_and_call_follow_the_pinned_contract() {
        let compatibility: Value =
            serde_json::from_str(COMPATIBILITY_FIXTURE).expect("compatibility fixture parses");
        assert_eq!(compatibility["protocolVersion"], MCP_PROTOCOL_VERSION);
        let server = FakeServer::start(success_responses());
        let run_dir = fixture_run_dir(server.address);
        let client = ContextStillRecallClient::with_run_dir(run_dir.path().to_path_buf(), true);
        assert!(client.is_configured());

        let result = client
            .recall(RECALL_RULE_TOOL_NAME, r#"{"query":"release"}"#)
            .await
            .expect("recall succeeds");
        assert_eq!(
            serde_json::from_str::<Value>(&result).expect("result is JSON"),
            serde_json::from_str::<Value>(RULE_FIXTURE).expect("fixture is JSON")
        );

        let captures = server.join();
        assert_eq!(captures.len(), 4);
        for (index, capture) in captures.iter().enumerate() {
            let lower = capture.to_ascii_lowercase();
            assert!(lower.contains("authorization: bearer "));
            assert!(lower.contains("accept: application/json, text/event-stream"));
            assert!(lower.contains("content-type: application/json"));
            if index > 0 {
                assert!(lower.contains("mcp-session-id: session-test"));
            } else {
                assert!(!lower.contains("mcp-session-id:"));
            }
        }
        assert!(captures[0].contains("\"method\":\"initialize\""));
        assert!(captures[1].contains("\"method\":\"notifications/initialized\""));
        assert!(captures[2].contains("\"method\":\"tools/list\""));
        assert!(captures[3].contains("\"method\":\"tools/call\""));
        assert!(!captures[3].contains("projectRef"));
    }
#[tokio::test]
    pub(super) async fn live_session_is_reused_without_reinitializing() {
        let mut responses = success_responses();
        responses.push(tool_call_response(4));
        let server = FakeServer::start(responses);
        let run_dir = fixture_run_dir(server.address);
        let client = ContextStillRecallClient::with_run_dir(run_dir.path().to_path_buf(), true);

        for query in ["first", "second"] {
            client
                .recall(RECALL_RULE_TOOL_NAME, &json!({"query": query}).to_string())
                .await
                .expect("recall succeeds");
        }
        let captures = server.join();
        assert_eq!(captures.len(), 5);
        assert_eq!(
            captures
                .iter()
                .filter(|request| request.contains("\"method\":\"initialize\""))
                .count(),
            1
        );
        assert!(captures[4].contains("\"id\":4"));
    }
#[tokio::test]
    pub(super) async fn expired_session_reinitializes_and_retries_once() {
        let mut responses = success_responses();
        responses.pop();
        responses.push(empty_response("404 Not Found"));
        responses.extend(success_responses());
        let server = FakeServer::start(responses);
        let run_dir = fixture_run_dir(server.address);
        let client = ContextStillRecallClient::with_run_dir(run_dir.path().to_path_buf(), true);

        client
            .recall(RECALL_RULE_TOOL_NAME, r#"{"query":"release"}"#)
            .await
            .expect("retry succeeds");
        let captures = server.join();
        assert_eq!(captures.len(), 8);
        assert_eq!(
            captures
                .iter()
                .filter(|request| request.contains("\"method\":\"initialize\""))
                .count(),
            2
        );
    }
#[tokio::test]
    pub(super) async fn rpc_session_error_reinitializes_and_retries_once() {
        let mut responses = success_responses();
        responses.pop();
        responses.push(response(
            "200 OK",
            &json!({
                "jsonrpc": "2.0",
                "id": 3,
                "error": {"code": -32000, "message": "session expired"}
            }),
            None,
        ));
        responses.extend(success_responses());
        let server = FakeServer::start(responses);
        let run_dir = fixture_run_dir(server.address);
        let client = ContextStillRecallClient::with_run_dir(run_dir.path().to_path_buf(), true);

        client
            .recall(RECALL_RULE_TOOL_NAME, r#"{"query":"release"}"#)
            .await
            .expect("RPC session error retry succeeds");
        let captures = server.join();
        assert_eq!(captures.len(), 8);
        assert_eq!(
            captures
                .iter()
                .filter(|request| request.contains("\"method\":\"initialize\""))
                .count(),
            2
        );
    }
#[tokio::test]
    pub(super) async fn session_expiry_during_initialize_lifecycle_retries_once() {
        let mut responses = success_responses();
        responses.truncate(1);
        responses.push(empty_response("404 Not Found"));
        responses.extend(success_responses());
        let server = FakeServer::start(responses);
        let run_dir = fixture_run_dir(server.address);
        let client = ContextStillRecallClient::with_run_dir(run_dir.path().to_path_buf(), true);

        client
            .recall(RECALL_RULE_TOOL_NAME, r#"{"query":"release"}"#)
            .await
            .expect("lifecycle retry succeeds");
        let captures = server.join();
        assert_eq!(captures.len(), 6);
        assert_eq!(
            captures
                .iter()
                .filter(|request| request.contains("\"method\":\"initialize\""))
                .count(),
            2
        );
    }
#[tokio::test]
    pub(super) async fn contract_block_clears_only_after_the_manifest_identifies_a_restart() {
        let mut malformed_responses = success_responses();
        malformed_responses.pop();
        malformed_responses.push(response(
            "200 OK",
            &json!({
                "jsonrpc": "2.0",
                "id": 3,
                "result": {"content": [{"type": "text", "text": "{}"}]}
            }),
            None,
        ));
        let first_server = FakeServer::start(malformed_responses);
        let run_dir = fixture_run_dir(first_server.address);
        let client = ContextStillRecallClient::with_run_dir(run_dir.path().to_path_buf(), true);

        assert_eq!(
            client
                .recall(RECALL_RULE_TOOL_NAME, r#"{"query":"release"}"#)
                .await,
            Err(ContextStillRecallError::InvalidResponse)
        );
        drop(first_server.join());
        assert!(!client.is_configured());

        let restarted_server = FakeServer::start(success_responses());
        write_fixture_manifest(run_dir.path(), restarted_server.address, "unix-ms:2");
        assert!(client.is_configured());
        client
            .recall(RECALL_RULE_TOOL_NAME, r#"{"query":"release"}"#)
            .await
            .expect("a new manifest permits a fresh audited session");
        drop(restarted_server.join());
    }
#[tokio::test]
    pub(super) async fn oversized_error_bodies_remain_transport_failures_and_do_not_block_the_contract() {
        let body = "x".repeat(MAX_HTTP_RESPONSE_BYTES + 1);
        let oversized_error = format!(
            "HTTP/1.1 500 Internal Server Error\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        let server = FakeServer::start(vec![oversized_error]);
        let run_dir = fixture_run_dir(server.address);
        let client = ContextStillRecallClient::with_run_dir(run_dir.path().to_path_buf(), true);

        assert_eq!(
            client
                .recall(RECALL_RULE_TOOL_NAME, r#"{"query":"release"}"#)
                .await,
            Err(ContextStillRecallError::Transport)
        );
        drop(server.join());
        assert!(client.is_configured());
    }
#[test]
    pub(super) fn manifest_rejects_default_profile_remote_url_and_secret_fields() {
        let server = FakeServer::start(Vec::new());
        let run_dir = fixture_run_dir(server.address);
        let manifest_path = run_dir.path().join(ENDPOINT_MANIFEST_FILE);
        let original: Value =
            serde_json::from_slice(&std::fs::read(&manifest_path).expect("manifest reads"))
                .expect("manifest parses");
        let mut default_profile = original.clone();
        default_profile["toolProfile"] = json!("default");
        let mut remote_url = original.clone();
        remote_url["url"] = json!("http://192.0.2.1:39173/mcp");
        let mut embedded_secret = original;
        embedded_secret["token"] = json!(TEST_TOKEN);

        for manifest in [default_profile, remote_url, embedded_secret] {
            std::fs::write(
                &manifest_path,
                serde_json::to_vec(&manifest).expect("manifest encodes"),
            )
            .expect("manifest writes");
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&manifest_path, std::fs::Permissions::from_mode(0o600))
                    .expect("permissions set");
            }
            let client = ContextStillRecallClient::with_run_dir(run_dir.path().to_path_buf(), true);
            assert!(!client.is_configured());
        }
        drop(server.join());
    }
#[test]
    pub(super) fn configuration_requires_an_absolute_run_dir_valid_start_marker_and_token() {
        assert_eq!(
            load_manifest(Path::new("relative-run-directory")),
            Err(ContextStillRecallError::Configuration)
        );

        let server = FakeServer::start(Vec::new());
        let run_dir = fixture_run_dir(server.address);
        let token_path = run_dir.path().join("mcp-memory-bearer.token");
        std::fs::write(&token_path, "short-token\n").expect("invalid token writes");
        let client = ContextStillRecallClient::with_run_dir(run_dir.path().to_path_buf(), true);
        assert!(!client.is_configured());

        std::fs::write(&token_path, format!("{TEST_TOKEN}\n\n")).expect("ambiguous token writes");
        assert!(!client.is_configured());

        std::fs::write(&token_path, format!("{TEST_TOKEN}\n")).expect("valid token writes");
        write_fixture_manifest(run_dir.path(), server.address, "not-a-start-marker");
        assert!(!client.is_configured());
        drop(server.join());
    }
#[cfg(unix)]
    #[test]
    pub(super) fn configuration_rejects_group_or_world_accessible_runtime_secrets() {
        use std::os::unix::fs::PermissionsExt;

        let server = FakeServer::start(Vec::new());
        let run_dir = fixture_run_dir(server.address);
        let manifest_path = run_dir.path().join(ENDPOINT_MANIFEST_FILE);
        let token_path = run_dir.path().join("mcp-memory-bearer.token");

        std::fs::set_permissions(&manifest_path, std::fs::Permissions::from_mode(0o644))
            .expect("manifest permissions change");
        let client = ContextStillRecallClient::with_run_dir(run_dir.path().to_path_buf(), true);
        assert!(!client.is_configured());

        std::fs::set_permissions(&manifest_path, std::fs::Permissions::from_mode(0o600))
            .expect("manifest permissions restore");
        std::fs::set_permissions(&token_path, std::fs::Permissions::from_mode(0o640))
            .expect("token permissions change");
        assert!(!client.is_configured());

        std::fs::set_permissions(&token_path, std::fs::Permissions::from_mode(0o600))
            .expect("token permissions restore");
        std::fs::set_permissions(run_dir.path(), std::fs::Permissions::from_mode(0o750))
            .expect("run directory permissions change");
        assert!(!client.is_configured());
        drop(server.join());
    }
#[test]
    pub(super) fn invalid_arguments_fail_before_token_or_network_access() {
        let client = ContextStillRecallClient::with_run_dir(PathBuf::from("/missing"), true);
        let runtime = tokio::runtime::Runtime::new().expect("runtime creates");
        let result = runtime.block_on(client.recall(
            RECALL_RULE_TOOL_NAME,
            r#"{"query":"release","projectRef":"forbidden"}"#,
        ));
        assert_eq!(result, Err(ContextStillRecallError::InvalidInput));
    }
#[test]
    pub(super) fn tool_catalog_accepts_only_the_exact_set_regardless_of_order() {
        let reordered = json!({
            "tools": [
                {"name": "recall_skill", "inputSchema": typed_recall_input_schema("recall_skill")},
                {"name": "recall_experience", "inputSchema": typed_recall_input_schema("recall_experience")},
                {"name": "recall_rule", "inputSchema": typed_recall_input_schema("recall_rule")}
            ]
        });
        assert_eq!(validate_tool_catalog(&reordered), Ok(()));

        let duplicate = json!({
            "tools": [
                {"name": "recall_experience", "inputSchema": typed_recall_input_schema("recall_experience")},
                {"name": "recall_rule", "inputSchema": typed_recall_input_schema("recall_rule")},
                {"name": "recall_rule", "inputSchema": typed_recall_input_schema("recall_rule")}
            ]
        });
        assert_eq!(
            validate_tool_catalog(&duplicate),
            Err(ContextStillRecallError::Protocol)
        );

        let mut drifted = reordered;
        drifted["tools"][0]["inputSchema"]["additionalProperties"] = json!(true);
        assert_eq!(
            validate_tool_catalog(&drifted),
            Err(ContextStillRecallError::Protocol)
        );
    }
