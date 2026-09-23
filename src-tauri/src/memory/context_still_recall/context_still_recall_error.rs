use super::*;
pub(super) const MCP_PROTOCOL_VERSION: &str = "2025-03-26";
const MCP_ACCEPT: &str = "application/json, text/event-stream";
const MCP_SESSION_HEADER: &str = "Mcp-Session-Id";
pub(super) const ENDPOINT_MANIFEST_FILE: &str = "mcp-endpoint.json";
pub(super) const MAX_MANIFEST_BYTES: u64 = 4 * 1_024;
pub(super) const MAX_TOKEN_BYTES: u64 = 128;
pub(super) const MAX_HTTP_RESPONSE_BYTES: usize = 16 * 1_024;
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
    pub(super) inner: Arc<ClientInner>,
}
pub(super) struct ClientInner {
    pub(super) enabled: bool,
    pub(super) run_dir: PathBuf,
    pub(super) http: Option<Client>,
    pub(super) session: Mutex<Option<SessionState>>,
    pub(super) contract_blocked_manifest: RwLock<Option<EndpointManifest>>,
}
pub(super) struct SessionState {
    pub(super) manifest: EndpointManifest,
    pub(super) token: Zeroizing<String>,
    pub(super) session_id: Zeroizing<String>,
    pub(super) last_used: tokio::time::Instant,
    pub(super) next_request_id: u64,
}
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct EndpointManifest {
    pub(super) server: String,
    pub(super) url: String,
    pub(super) transport: String,
    pub(super) protocol_version: String,
    pub(super) auth: String,
    pub(super) auth_token_path: PathBuf,
    pub(super) tool_profile: String,
    pub(super) contract_version: String,
    pub(super) started_at: String,
}
impl ContextStillRecallClient {
    pub fn from_environment() -> Self {
        Self::with_run_dir(resolve_run_dir(), control_plane::memory_enabled())
    }

    #[cfg(any(test, feature = "quality-eval-harness"))]
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
    pub(super) status: StatusCode,
    pub(super) headers: HeaderMap,
    pub(super) body: Vec<u8>,
}
pub(super) fn session_is_reusable(session: &SessionState, manifest: &EndpointManifest) -> bool {
    session.manifest == *manifest && session.last_used.elapsed() < SESSION_REUSE_WINDOW
}
pub(super) fn tool_name(memory_type: TypedMemoryType) -> &'static str {
    match memory_type {
        TypedMemoryType::Experience => TYPED_RECALL_TOOL_NAMES[0],
        TypedMemoryType::Rule => TYPED_RECALL_TOOL_NAMES[1],
        TypedMemoryType::Skill => TYPED_RECALL_TOOL_NAMES[2],
    }
}
pub(super) fn parse_rpc_result(
    body: &[u8],
    expected_id: u64,
) -> Result<Value, ContextStillRecallError> {
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
pub(super) fn validate_tool_catalog(result: &Value) -> Result<(), ContextStillRecallError> {
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
pub(super) fn require_json_content_type(
    headers: &HeaderMap,
) -> Result<(), ContextStillRecallError> {
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
pub(super) async fn read_body_limited(
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
pub(super) fn resolve_run_dir() -> PathBuf {
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
