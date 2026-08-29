use std::{
    collections::BTreeSet,
    env, fs,
    io::Read,
    path::{Path, PathBuf},
    sync::{Arc, RwLock},
    time::Duration,
};

use futures_util::StreamExt;
use reqwest::{
    header::{HeaderMap, ACCEPT, CONTENT_TYPE},
    Client, StatusCode,
};
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::sync::Mutex;
use url::{Host, Url};
use zeroize::Zeroizing;

#[cfg(test)]
use super::typed_recall::RECALL_RULE_TOOL_NAME;
use super::{
    control_plane,
    typed_recall::{
        parse_call_tool_result, parse_typed_recall_arguments, typed_recall_input_schema,
        TypedMemoryType, TypedRecallContractError, ValidatedTypedRecallCall,
        MEMORY_RECALL_CONTRACT_VERSION, TYPED_RECALL_TOOL_NAMES,
    },
};

const MCP_PROTOCOL_VERSION: &str = "2025-03-26";
const MCP_ACCEPT: &str = "application/json, text/event-stream";
const MCP_SESSION_HEADER: &str = "Mcp-Session-Id";
const ENDPOINT_MANIFEST_FILE: &str = "mcp-endpoint.json";
const MAX_MANIFEST_BYTES: u64 = 4 * 1_024;
const MAX_TOKEN_BYTES: u64 = 128;
const MAX_HTTP_RESPONSE_BYTES: usize = 16 * 1_024;
const MAX_HTTP_REQUEST_BYTES: usize = 16 * 1_024;
const SESSION_REUSE_WINDOW: Duration = Duration::from_secs(55);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContextStillRecallError {
    Disabled,
    Configuration,
    Authentication,
    Transport,
    Protocol,
    InvalidInput,
    InvalidResponse,
    ResponseTooLarge,
}

impl ContextStillRecallError {
    pub const fn tool_code(self) -> &'static str {
        match self {
            Self::InvalidInput => "invalid-memory-input",
            Self::ResponseTooLarge => "typed-memory-response-too-large",
            Self::Protocol | Self::InvalidResponse => "typed-memory-contract-error",
            Self::Disabled | Self::Configuration | Self::Authentication | Self::Transport => {
                "typed-memory-unavailable"
            }
        }
    }

    pub const fn safe_message(self) -> &'static str {
        match self {
            Self::InvalidInput => "Memory recall arguments do not match the selected tool schema.",
            Self::ResponseTooLarge => {
                "The typed memory response exceeded its fixed contract limit."
            }
            Self::Protocol | Self::InvalidResponse => {
                "The typed memory response did not match memory-recall-v1."
            }
            Self::Disabled | Self::Configuration | Self::Authentication | Self::Transport => {
                "Typed memory recall is temporarily unavailable."
            }
        }
    }
}

impl From<TypedRecallContractError> for ContextStillRecallError {
    fn from(error: TypedRecallContractError) -> Self {
        match error {
            TypedRecallContractError::UnsupportedTool | TypedRecallContractError::InvalidInput => {
                Self::InvalidInput
            }
            TypedRecallContractError::InvalidResponse => Self::InvalidResponse,
            TypedRecallContractError::ResponseTooLarge => Self::ResponseTooLarge,
        }
    }
}

#[derive(Clone)]
pub struct ContextStillRecallClient {
    inner: Arc<ClientInner>,
}

struct ClientInner {
    enabled: bool,
    run_dir: PathBuf,
    http: Option<Client>,
    session: Mutex<Option<SessionState>>,
    contract_blocked_manifest: RwLock<Option<EndpointManifest>>,
}

struct SessionState {
    manifest: EndpointManifest,
    token: Zeroizing<String>,
    session_id: Zeroizing<String>,
    last_used: tokio::time::Instant,
    next_request_id: u64,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct EndpointManifest {
    server: String,
    url: String,
    transport: String,
    protocol_version: String,
    auth: String,
    auth_token_path: PathBuf,
    tool_profile: String,
    contract_version: String,
    started_at: String,
}

impl ContextStillRecallClient {
    pub fn from_environment() -> Self {
        Self::with_run_dir(resolve_run_dir(), control_plane::memory_enabled())
    }

    #[cfg(test)]
    pub fn disabled() -> Self {
        Self::with_run_dir(PathBuf::new(), false)
    }

    pub(crate) fn with_run_dir(run_dir: PathBuf, enabled: bool) -> Self {
        let http = enabled
            .then(|| {
                Client::builder()
                    .timeout(Duration::from_secs(3))
                    .redirect(reqwest::redirect::Policy::none())
                    .no_proxy()
                    .build()
                    .ok()
            })
            .flatten();
        Self {
            inner: Arc::new(ClientInner {
                enabled,
                run_dir,
                http,
                session: Mutex::new(None),
                contract_blocked_manifest: RwLock::new(None),
            }),
        }
    }

    pub fn is_configured(&self) -> bool {
        if !self.inner.enabled || self.inner.http.is_none() {
            return false;
        }
        let Ok(manifest) = load_manifest(&self.inner.run_dir) else {
            return false;
        };
        !self.contract_is_blocked_for(&manifest)
            && read_token(&self.inner.run_dir, &manifest.auth_token_path).is_ok()
    }

    pub async fn recall(
        &self,
        tool_name: &str,
        arguments: &str,
    ) -> Result<String, ContextStillRecallError> {
        if !self.inner.enabled {
            return Err(ContextStillRecallError::Disabled);
        }
        let call = parse_typed_recall_arguments(tool_name, arguments)?;
        let mut session = self.inner.session.lock().await;
        for attempt in 0..=1 {
            let manifest = load_manifest(&self.inner.run_dir)?;
            if self.contract_is_blocked_for(&manifest) {
                return Err(ContextStillRecallError::Protocol);
            }
            if !session
                .as_ref()
                .is_some_and(|current| session_is_reusable(current, &manifest))
            {
                match self.initialize_session(manifest.clone()).await {
                    Ok(initialized) => *session = Some(initialized),
                    Err(ContextStillRecallError::Authentication) if attempt == 0 => {
                        *session = None;
                        continue;
                    }
                    Err(error) => {
                        self.block_if_contract_error(error, &manifest);
                        return Err(error);
                    }
                }
            }
            let current = session.as_mut().ok_or(ContextStillRecallError::Protocol)?;
            let current_manifest = current.manifest.clone();
            match self.call_tool(current, &call).await {
                Ok(result) => return Ok(result),
                Err(CallFailure::SessionExpired) if attempt == 0 => {
                    *session = None;
                }
                Err(CallFailure::Error(error)) => {
                    self.block_if_contract_error(error, &current_manifest);
                    return Err(error);
                }
                Err(CallFailure::SessionExpired) => {
                    *session = None;
                    return Err(ContextStillRecallError::Authentication);
                }
            }
        }
        Err(ContextStillRecallError::Authentication)
    }

    fn contract_is_blocked_for(&self, manifest: &EndpointManifest) -> bool {
        self.inner
            .contract_blocked_manifest
            .read()
            .map(|blocked| blocked.as_ref() == Some(manifest))
            .unwrap_or(true)
    }

    fn block_if_contract_error(&self, error: ContextStillRecallError, manifest: &EndpointManifest) {
        if matches!(
            error,
            ContextStillRecallError::Protocol
                | ContextStillRecallError::InvalidResponse
                | ContextStillRecallError::ResponseTooLarge
        ) {
            if let Ok(mut blocked) = self.inner.contract_blocked_manifest.write() {
                *blocked = Some(manifest.clone());
            }
        }
    }

    async fn initialize_session(
        &self,
        manifest: EndpointManifest,
    ) -> Result<SessionState, ContextStillRecallError> {
        let token = read_token(&self.inner.run_dir, &manifest.auth_token_path)?;
        let initialize = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": MCP_PROTOCOL_VERSION,
                "capabilities": {},
                "clientInfo": {"name": "saaa-desktop", "version": env!("CARGO_PKG_VERSION")}
            }
        });
        let response = self.post_json(&manifest, &token, None, &initialize).await?;
        if response.status == StatusCode::UNAUTHORIZED {
            return Err(ContextStillRecallError::Authentication);
        }
        if response.status != StatusCode::OK {
            return Err(ContextStillRecallError::Transport);
        }
        require_json_content_type(&response.headers)?;
        let session_id = response
            .headers
            .get(MCP_SESSION_HEADER)
            .and_then(|value| value.to_str().ok())
            .filter(|value| valid_session_id(value))
            .ok_or(ContextStillRecallError::Protocol)?
            .to_string();
        let initialize_result = parse_rpc_result(&response.body, 1)?;
        if initialize_result
            .get("protocolVersion")
            .and_then(Value::as_str)
            != Some(MCP_PROTOCOL_VERSION)
            || !initialize_result
                .pointer("/capabilities/tools")
                .is_some_and(Value::is_object)
        {
            return Err(ContextStillRecallError::Protocol);
        }

        let initialized = json!({
            "jsonrpc": "2.0",
            "method": "notifications/initialized",
            "params": {}
        });
        let response = self
            .post_json(&manifest, &token, Some(&session_id), &initialized)
            .await?;
        if matches!(
            response.status,
            StatusCode::UNAUTHORIZED | StatusCode::NOT_FOUND
        ) {
            return Err(ContextStillRecallError::Authentication);
        }
        if response.status != StatusCode::ACCEPTED || !response.body.is_empty() {
            return Err(ContextStillRecallError::Protocol);
        }

        let list = json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list", "params": {}});
        let response = self
            .post_json(&manifest, &token, Some(&session_id), &list)
            .await?;
        if matches!(
            response.status,
            StatusCode::UNAUTHORIZED | StatusCode::NOT_FOUND
        ) {
            return Err(ContextStillRecallError::Authentication);
        }
        if response.status != StatusCode::OK {
            return Err(ContextStillRecallError::Transport);
        }
        require_json_content_type(&response.headers)?;
        let list_result = parse_rpc_result(&response.body, 2)?;
        validate_tool_catalog(&list_result)?;

        Ok(SessionState {
            manifest,
            token,
            session_id: Zeroizing::new(session_id),
            last_used: tokio::time::Instant::now(),
            next_request_id: 3,
        })
    }

    async fn call_tool(
        &self,
        session: &mut SessionState,
        call: &ValidatedTypedRecallCall,
    ) -> Result<String, CallFailure> {
        let request_id = session.next_request_id;
        session.next_request_id = session.next_request_id.saturating_add(1);
        let request = json!({
            "jsonrpc": "2.0",
            "id": request_id,
            "method": "tools/call",
            "params": {
                "name": tool_name(call.memory_type),
                "arguments": &call.arguments
            }
        });
        let response = self
            .post_json(
                &session.manifest,
                &session.token,
                Some(&session.session_id),
                &request,
            )
            .await
            .map_err(CallFailure::Error)?;
        if matches!(
            response.status,
            StatusCode::UNAUTHORIZED | StatusCode::NOT_FOUND
        ) {
            return Err(CallFailure::SessionExpired);
        }
        if response.status != StatusCode::OK {
            return Err(CallFailure::Error(ContextStillRecallError::Transport));
        }
        require_json_content_type(&response.headers).map_err(CallFailure::Error)?;
        let result = parse_rpc_result(&response.body, request_id).map_err(|error| {
            if error == ContextStillRecallError::Authentication {
                CallFailure::SessionExpired
            } else {
                CallFailure::Error(error)
            }
        })?;
        let result = parse_call_tool_result(call.memory_type, &result)
            .map_err(|error| CallFailure::Error(ContextStillRecallError::from(error)))?;
        session.last_used = tokio::time::Instant::now();
        Ok(result)
    }

    async fn post_json(
        &self,
        manifest: &EndpointManifest,
        token: &str,
        session_id: Option<&str>,
        body: &Value,
    ) -> Result<HttpResponse, ContextStillRecallError> {
        let http = self
            .inner
            .http
            .as_ref()
            .ok_or(ContextStillRecallError::Disabled)?;
        let encoded = serde_json::to_vec(body).map_err(|_| ContextStillRecallError::Protocol)?;
        if encoded.len() > MAX_HTTP_REQUEST_BYTES {
            return Err(ContextStillRecallError::ResponseTooLarge);
        }
        let mut request = http
            .post(&manifest.url)
            .bearer_auth(token)
            .header(ACCEPT, MCP_ACCEPT)
            .header(CONTENT_TYPE, "application/json");
        if let Some(session_id) = session_id {
            request = request.header(MCP_SESSION_HEADER, session_id);
        }
        let response = request
            .body(encoded)
            .send()
            .await
            .map_err(|_| ContextStillRecallError::Transport)?;
        let status = response.status();
        let headers = response.headers().clone();
        let body = if status.is_success() {
            read_body_limited(response, MAX_HTTP_RESPONSE_BYTES).await?
        } else {
            Vec::new()
        };
        Ok(HttpResponse {
            status,
            headers,
            body,
        })
    }
}

enum CallFailure {
    SessionExpired,
    Error(ContextStillRecallError),
}

struct HttpResponse {
    status: StatusCode,
    headers: HeaderMap,
    body: Vec<u8>,
}

fn session_is_reusable(session: &SessionState, manifest: &EndpointManifest) -> bool {
    session.manifest == *manifest && session.last_used.elapsed() < SESSION_REUSE_WINDOW
}

fn tool_name(memory_type: TypedMemoryType) -> &'static str {
    match memory_type {
        TypedMemoryType::Experience => TYPED_RECALL_TOOL_NAMES[0],
        TypedMemoryType::Rule => TYPED_RECALL_TOOL_NAMES[1],
        TypedMemoryType::Skill => TYPED_RECALL_TOOL_NAMES[2],
    }
}

fn parse_rpc_result(body: &[u8], expected_id: u64) -> Result<Value, ContextStillRecallError> {
    let value: Value =
        serde_json::from_slice(body).map_err(|_| ContextStillRecallError::Protocol)?;
    let object = value.as_object().ok_or(ContextStillRecallError::Protocol)?;
    if object.get("jsonrpc").and_then(Value::as_str) != Some("2.0")
        || object.get("id").and_then(Value::as_u64) != Some(expected_id)
    {
        return Err(ContextStillRecallError::Protocol);
    }
    if let Some(error) = object.get("error") {
        return match error.get("code").and_then(Value::as_i64) {
            Some(-32603) => Err(ContextStillRecallError::Transport),
            Some(-32000) => Err(ContextStillRecallError::Authentication),
            _ => Err(ContextStillRecallError::Protocol),
        };
    }
    object
        .get("result")
        .cloned()
        .ok_or(ContextStillRecallError::Protocol)
}

fn validate_tool_catalog(result: &Value) -> Result<(), ContextStillRecallError> {
    let tools = result
        .get("tools")
        .and_then(Value::as_array)
        .ok_or(ContextStillRecallError::Protocol)?;
    if tools.len() != TYPED_RECALL_TOOL_NAMES.len() {
        return Err(ContextStillRecallError::Protocol);
    }
    let names = tools
        .iter()
        .map(|tool| {
            let name = tool
                .get("name")
                .and_then(Value::as_str)
                .ok_or(ContextStillRecallError::Protocol)?;
            if tool.get("inputSchema") != typed_recall_input_schema(name).as_ref() {
                return Err(ContextStillRecallError::Protocol);
            }
            Ok(name)
        })
        .collect::<Result<BTreeSet<_>, _>>()?;
    let expected = TYPED_RECALL_TOOL_NAMES.into_iter().collect::<BTreeSet<_>>();
    if names != expected {
        return Err(ContextStillRecallError::Protocol);
    }
    Ok(())
}

fn require_json_content_type(headers: &HeaderMap) -> Result<(), ContextStillRecallError> {
    let content_type = headers
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(';').next())
        .map(str::trim);
    if content_type == Some("application/json") {
        Ok(())
    } else {
        Err(ContextStillRecallError::Protocol)
    }
}

async fn read_body_limited(
    response: reqwest::Response,
    limit: usize,
) -> Result<Vec<u8>, ContextStillRecallError> {
    if response
        .content_length()
        .is_some_and(|length| length > limit as u64)
    {
        return Err(ContextStillRecallError::ResponseTooLarge);
    }
    let mut body = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|_| ContextStillRecallError::Transport)?;
        if body.len().saturating_add(chunk.len()) > limit {
            return Err(ContextStillRecallError::ResponseTooLarge);
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

fn resolve_run_dir() -> PathBuf {
    if let Some(path) = env::var_os("SAAA_CONTEXT_STILL_RUN_DIR").filter(|path| !path.is_empty()) {
        return PathBuf::from(path);
    }
    if let Some(path) = env::var_os("CONTEXT_STILL_APP_DATA_DIR").filter(|path| !path.is_empty()) {
        return PathBuf::from(path).join("run");
    }
    #[cfg(target_os = "macos")]
    if let Some(home) = env::var_os("HOME") {
        return PathBuf::from(home)
            .join("Library")
            .join("Application Support")
            .join("contextStill")
            .join("run");
    }
    #[cfg(target_os = "windows")]
    if let Some(app_data) = env::var_os("APPDATA") {
        return PathBuf::from(app_data).join("contextStill").join("run");
    }
    if let Some(data_home) = env::var_os("XDG_DATA_HOME") {
        return PathBuf::from(data_home).join("contextStill").join("run");
    }
    env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".local")
        .join("share")
        .join("contextStill")
        .join("run")
}

fn load_manifest(run_dir: &Path) -> Result<EndpointManifest, ContextStillRecallError> {
    if !run_dir.is_absolute() {
        return Err(ContextStillRecallError::Configuration);
    }
    validate_owner_only_directory(run_dir)?;
    let path = run_dir.join(ENDPOINT_MANIFEST_FILE);
    validate_owner_only_file(&path)?;
    let content = read_file_limited(&path, MAX_MANIFEST_BYTES)?;
    let manifest: EndpointManifest =
        serde_json::from_slice(&content).map_err(|_| ContextStillRecallError::Configuration)?;
    validate_manifest(run_dir, &manifest)?;
    Ok(manifest)
}

fn validate_manifest(
    run_dir: &Path,
    manifest: &EndpointManifest,
) -> Result<(), ContextStillRecallError> {
    if manifest.server != "context-still"
        || manifest.transport != "streamable-http"
        || manifest.protocol_version != MCP_PROTOCOL_VERSION
        || manifest.auth != "bearer-token-file"
        || manifest.tool_profile != "typed-memory"
        || manifest.contract_version != MEMORY_RECALL_CONTRACT_VERSION
        || !valid_started_at(&manifest.started_at)
    {
        return Err(ContextStillRecallError::Configuration);
    }
    let url = Url::parse(&manifest.url).map_err(|_| ContextStillRecallError::Configuration)?;
    let loopback = match url.host() {
        Some(Host::Ipv4(address)) => address.is_loopback(),
        Some(Host::Ipv6(address)) => address.is_loopback(),
        _ => false,
    };
    if url.scheme() != "http"
        || !loopback
        || url.path() != "/mcp"
        || url.query().is_some()
        || url.fragment().is_some()
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return Err(ContextStillRecallError::Configuration);
    }
    validate_token_path(run_dir, &manifest.auth_token_path)
}

fn valid_started_at(value: &str) -> bool {
    value
        .strip_prefix("unix-ms:")
        .filter(|millis| !millis.is_empty() && millis.bytes().all(|byte| byte.is_ascii_digit()))
        .and_then(|millis| millis.parse::<u64>().ok())
        .is_some()
}

fn validate_token_path(run_dir: &Path, token_path: &Path) -> Result<(), ContextStillRecallError> {
    if !token_path.is_absolute() {
        return Err(ContextStillRecallError::Configuration);
    }
    validate_owner_only_file(token_path)?;
    let canonical_run =
        fs::canonicalize(run_dir).map_err(|_| ContextStillRecallError::Configuration)?;
    let token_parent = token_path
        .parent()
        .ok_or(ContextStillRecallError::Configuration)?;
    let canonical_parent =
        fs::canonicalize(token_parent).map_err(|_| ContextStillRecallError::Configuration)?;
    if canonical_parent != canonical_run {
        return Err(ContextStillRecallError::Configuration);
    }
    Ok(())
}

fn read_token(run_dir: &Path, path: &Path) -> Result<Zeroizing<String>, ContextStillRecallError> {
    validate_token_path(run_dir, path)?;
    let content = Zeroizing::new(read_file_limited(path, MAX_TOKEN_BYTES)?);
    let token_bytes = match content.as_slice() {
        bytes if bytes.len() == 64 => bytes,
        bytes if bytes.len() == 65 && bytes.last() == Some(&b'\n') => &bytes[..64],
        bytes if bytes.len() == 66 && bytes.ends_with(b"\r\n") => &bytes[..64],
        _ => return Err(ContextStillRecallError::Authentication),
    };
    let token =
        std::str::from_utf8(token_bytes).map_err(|_| ContextStillRecallError::Authentication)?;
    if token.len() != 64 || !token.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(ContextStillRecallError::Authentication);
    }
    Ok(Zeroizing::new(token.to_string()))
}

fn read_file_limited(path: &Path, limit: u64) -> Result<Vec<u8>, ContextStillRecallError> {
    let file = fs::File::open(path).map_err(|_| ContextStillRecallError::Configuration)?;
    let metadata = file
        .metadata()
        .map_err(|_| ContextStillRecallError::Configuration)?;
    if metadata.len() == 0 || metadata.len() > limit {
        return Err(ContextStillRecallError::Configuration);
    }
    let mut content = Vec::with_capacity(usize::try_from(metadata.len()).unwrap_or(0));
    file.take(limit.saturating_add(1))
        .read_to_end(&mut content)
        .map_err(|_| ContextStillRecallError::Configuration)?;
    if content.is_empty() || content.len() > limit as usize {
        return Err(ContextStillRecallError::Configuration);
    }
    Ok(content)
}

fn validate_owner_only_directory(path: &Path) -> Result<(), ContextStillRecallError> {
    let metadata =
        fs::symlink_metadata(path).map_err(|_| ContextStillRecallError::Configuration)?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(ContextStillRecallError::Configuration);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        if metadata.uid() != unsafe { libc::geteuid() }
            || metadata.permissions().mode() & 0o077 != 0
        {
            return Err(ContextStillRecallError::Configuration);
        }
    }
    Ok(())
}

fn validate_owner_only_file(path: &Path) -> Result<(), ContextStillRecallError> {
    let metadata =
        fs::symlink_metadata(path).map_err(|_| ContextStillRecallError::Configuration)?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(ContextStillRecallError::Configuration);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        if metadata.uid() != unsafe { libc::geteuid() }
            || metadata.permissions().mode() & 0o177 != 0
        {
            return Err(ContextStillRecallError::Configuration);
        }
    }
    Ok(())
}

fn valid_session_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 256
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~'))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::{Read, Write},
        net::TcpListener,
        sync::{Arc, Mutex as StdMutex},
        thread,
    };

    const RULE_FIXTURE: &str = include_str!("../../tests/fixtures/memory-recall-v1/rule.json");
    const COMPATIBILITY_FIXTURE: &str =
        include_str!("../../tests/fixtures/memory-recall-v1/saaa-compatibility.json");
    const TEST_TOKEN: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    struct FakeServer {
        address: std::net::SocketAddr,
        captures: Arc<StdMutex<Vec<String>>>,
        thread: Option<thread::JoinHandle<()>>,
    }

    impl FakeServer {
        fn start(responses: Vec<String>) -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").expect("server binds");
            let address = listener.local_addr().expect("address resolves");
            let captures = Arc::new(StdMutex::new(Vec::new()));
            let captures_for_thread = captures.clone();
            let thread = thread::spawn(move || {
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

    fn read_request(socket: &mut std::net::TcpStream) -> String {
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

    fn response(status: &str, body: &Value, session_id: Option<&str>) -> String {
        let body = serde_json::to_string(body).expect("body encodes");
        let session = session_id
            .map(|session| format!("Mcp-Session-Id: {session}\r\n"))
            .unwrap_or_default();
        format!(
            "HTTP/1.1 {status}\r\nContent-Type: application/json\r\n{session}Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
    }

    fn empty_response(status: &str) -> String {
        format!("HTTP/1.1 {status}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
    }

    fn success_responses() -> Vec<String> {
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

    fn tool_call_response(id: u64) -> String {
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

    fn fixture_run_dir(address: std::net::SocketAddr) -> tempfile::TempDir {
        let directory = tempfile::tempdir().expect("temp directory creates");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(directory.path(), fs::Permissions::from_mode(0o700))
                .expect("directory permissions set");
        }
        let token_path = directory.path().join("mcp-memory-bearer.token");
        fs::write(&token_path, format!("{TEST_TOKEN}\n")).expect("token writes");
        write_fixture_manifest(directory.path(), address, "unix-ms:1");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&token_path, fs::Permissions::from_mode(0o600))
                .expect("file permissions set");
        }
        directory
    }

    fn write_fixture_manifest(run_dir: &Path, address: std::net::SocketAddr, started_at: &str) {
        let token_path = run_dir.join("mcp-memory-bearer.token");
        let manifest_path = run_dir.join(ENDPOINT_MANIFEST_FILE);
        fs::write(
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
            fs::set_permissions(&manifest_path, fs::Permissions::from_mode(0o600))
                .expect("file permissions set");
        }
    }

    #[tokio::test]
    async fn lifecycle_auth_catalog_and_call_follow_the_pinned_contract() {
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
    async fn live_session_is_reused_without_reinitializing() {
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
    async fn expired_session_reinitializes_and_retries_once() {
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
    async fn rpc_session_error_reinitializes_and_retries_once() {
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
    async fn session_expiry_during_initialize_lifecycle_retries_once() {
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
    async fn contract_block_clears_only_after_the_manifest_identifies_a_restart() {
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
    async fn oversized_error_bodies_remain_transport_failures_and_do_not_block_the_contract() {
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
    fn manifest_rejects_default_profile_remote_url_and_secret_fields() {
        let server = FakeServer::start(Vec::new());
        let run_dir = fixture_run_dir(server.address);
        let manifest_path = run_dir.path().join(ENDPOINT_MANIFEST_FILE);
        let original: Value =
            serde_json::from_slice(&fs::read(&manifest_path).expect("manifest reads"))
                .expect("manifest parses");
        let mut default_profile = original.clone();
        default_profile["toolProfile"] = json!("default");
        let mut remote_url = original.clone();
        remote_url["url"] = json!("http://192.0.2.1:39173/mcp");
        let mut embedded_secret = original;
        embedded_secret["token"] = json!(TEST_TOKEN);

        for manifest in [default_profile, remote_url, embedded_secret] {
            fs::write(
                &manifest_path,
                serde_json::to_vec(&manifest).expect("manifest encodes"),
            )
            .expect("manifest writes");
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(&manifest_path, fs::Permissions::from_mode(0o600))
                    .expect("permissions set");
            }
            let client = ContextStillRecallClient::with_run_dir(run_dir.path().to_path_buf(), true);
            assert!(!client.is_configured());
        }
        drop(server.join());
    }

    #[test]
    fn configuration_requires_an_absolute_run_dir_valid_start_marker_and_token() {
        assert_eq!(
            load_manifest(Path::new("relative-run-directory")),
            Err(ContextStillRecallError::Configuration)
        );

        let server = FakeServer::start(Vec::new());
        let run_dir = fixture_run_dir(server.address);
        let token_path = run_dir.path().join("mcp-memory-bearer.token");
        fs::write(&token_path, "short-token\n").expect("invalid token writes");
        let client = ContextStillRecallClient::with_run_dir(run_dir.path().to_path_buf(), true);
        assert!(!client.is_configured());

        fs::write(&token_path, format!("{TEST_TOKEN}\n\n")).expect("ambiguous token writes");
        assert!(!client.is_configured());

        fs::write(&token_path, format!("{TEST_TOKEN}\n")).expect("valid token writes");
        write_fixture_manifest(run_dir.path(), server.address, "not-a-start-marker");
        assert!(!client.is_configured());
        drop(server.join());
    }

    #[cfg(unix)]
    #[test]
    fn configuration_rejects_group_or_world_accessible_runtime_secrets() {
        use std::os::unix::fs::PermissionsExt;

        let server = FakeServer::start(Vec::new());
        let run_dir = fixture_run_dir(server.address);
        let manifest_path = run_dir.path().join(ENDPOINT_MANIFEST_FILE);
        let token_path = run_dir.path().join("mcp-memory-bearer.token");

        fs::set_permissions(&manifest_path, fs::Permissions::from_mode(0o644))
            .expect("manifest permissions change");
        let client = ContextStillRecallClient::with_run_dir(run_dir.path().to_path_buf(), true);
        assert!(!client.is_configured());

        fs::set_permissions(&manifest_path, fs::Permissions::from_mode(0o600))
            .expect("manifest permissions restore");
        fs::set_permissions(&token_path, fs::Permissions::from_mode(0o640))
            .expect("token permissions change");
        assert!(!client.is_configured());

        fs::set_permissions(&token_path, fs::Permissions::from_mode(0o600))
            .expect("token permissions restore");
        fs::set_permissions(run_dir.path(), fs::Permissions::from_mode(0o750))
            .expect("run directory permissions change");
        assert!(!client.is_configured());
        drop(server.join());
    }

    #[test]
    fn invalid_arguments_fail_before_token_or_network_access() {
        let client = ContextStillRecallClient::with_run_dir(PathBuf::from("/missing"), true);
        let runtime = tokio::runtime::Runtime::new().expect("runtime creates");
        let result = runtime.block_on(client.recall(
            RECALL_RULE_TOOL_NAME,
            r#"{"query":"release","projectRef":"forbidden"}"#,
        ));
        assert_eq!(result, Err(ContextStillRecallError::InvalidInput));
    }

    #[test]
    fn tool_catalog_accepts_only_the_exact_set_regardless_of_order() {
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

    #[test]
    fn runtime_module_contains_no_logging_of_transport_data() {
        let source = include_str!("context_still_recall.rs");
        let forbidden = [
            ["print", "ln!"].concat(),
            ["eprint", "ln!"].concat(),
            ["db", "g!"].concat(),
            ["tracing", "::"].concat(),
            ["log", "::"].concat(),
        ];
        for forbidden in forbidden {
            assert!(
                !source.contains(&forbidden),
                "forbidden logger: {forbidden}"
            );
        }
    }

    #[tokio::test]
    #[ignore = "operator-only ContextStill typed-memory MCP compatibility canary"]
    async fn live_typed_memory_contract() {
        let client = ContextStillRecallClient::from_environment();
        assert!(client.is_configured());
        let result = client
            .recall(
                RECALL_RULE_TOOL_NAME,
                r#"{"query":"release health check","limit":1}"#,
            )
            .await
            .expect("live typed-memory recall follows memory-recall-v1");
        let value: Value = serde_json::from_str(&result).expect("result is strict JSON");
        assert_eq!(value["contractVersion"], MEMORY_RECALL_CONTRACT_VERSION);
        assert_eq!(value["memoryType"], "rule");
        assert_eq!(value["trust"]["instructionAuthority"], "none");
    }
}
