use std::{
    collections::HashMap,
    pin::Pin,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex as StdMutex, OnceLock,
    },
    time::Duration,
};

use futures_util::{stream::FuturesUnordered, Future, SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::{
    net::TcpStream,
    sync::{Mutex as AsyncMutex, OwnedSemaphorePermit, Semaphore},
};
use tokio_tungstenite::{
    connect_async_with_config,
    tungstenite::{
        client::IntoClientRequest,
        http::{header, HeaderValue},
        protocol::WebSocketConfig,
        Message,
    },
};
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

use super::protocol::{
    reject_duplicate_keys, validate_resume_negotiation, AcknowledgedCheckpoint, FinishReason,
    OrderedRun, ProtocolError, ProviderEvent, ResumeNegotiation, RunStart, SUBPROTOCOL,
};
use crate::{
    ipc_contract::RuntimeEvent,
    providers::stream::{execute_agent_tool, tool_was_offered, ProviderOutputPersistence},
    runtime::{agent_tools::AgentToolCall, event_hub::RuntimeEventSender},
    RunCancellation, StartTurnInput,
};

const MAX_SERVER_MESSAGE_BYTES: usize = 524_288;
const MAX_TOOL_RESULT_BYTES: usize = 262_144;
const ACK_EVENT_INTERVAL: u64 = 16;
const ACK_TIME_INTERVAL: Duration = Duration::from_millis(100);
const IDLE_HEALTH_TIMEOUT: Duration = Duration::from_millis(500);
const WRITE_TIMEOUT: Duration = Duration::from_secs(1);

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum WebSocketRunResult {
    Completed(String),
    Length(String),
    Failed { code: String, output_started: bool },
    Cancelled { output_started: bool },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WebSocketRunErrorKind {
    Authentication,
    Contract,
    Protocol,
    Network,
    Timeout,
    ClientDisconnected,
    Internal,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct WebSocketRunError {
    pub(crate) kind: WebSocketRunErrorKind,
    pub(crate) output_started: bool,
}

impl WebSocketRunError {
    const fn new(kind: WebSocketRunErrorKind) -> Self {
        Self {
            kind,
            output_started: false,
        }
    }

    const fn with_output_started(mut self, output_started: bool) -> Self {
        self.output_started |= output_started;
        self
    }
}

pub(crate) struct WebSocketRunContext<'a> {
    pub(crate) stream_url: &'a str,
    pub(crate) authorization: Option<&'a str>,
    pub(crate) allocation_id: Option<&'a str>,
    pub(crate) model: &'a str,
    pub(crate) messages: &'a [Value],
    pub(crate) tools: &'a [Value],
    pub(crate) reasoning_effort: &'a str,
    pub(crate) max_output_tokens: u32,
    pub(crate) tool_timeout: Duration,
    pub(crate) timeout: Duration,
    pub(crate) input: &'a StartTurnInput,
    pub(crate) on_event: &'a dyn RuntimeEventSender,
    pub(crate) cancellation: Arc<RunCancellation>,
    pub(crate) output_persistence: Option<ProviderOutputPersistence<'a>>,
}

type ToolFuture<'a> = Pin<Box<dyn Future<Output = ToolResult> + Send + 'a>>;

struct ToolResult {
    call_id: String,
    in_reply_to_seq: u64,
    content: String,
}

type ProviderSocket = WebSocketStream<MaybeTlsStream<TcpStream>>;

#[derive(Clone, Eq, Hash, PartialEq)]
struct PoolKey {
    stream_url: String,
    authorization_hash: [u8; 32],
}

struct ConnectionPool {
    state: AsyncMutex<PoolState>,
}

struct PoolEntry {
    pool: Arc<ConnectionPool>,
    generation: u64,
}

const MAX_CONNECTION_POOLS: usize = 16;

#[derive(Default)]
struct PoolState {
    idle: Vec<ProviderSocket>,
    semaphore: Option<Arc<Semaphore>>,
    max_connections: usize,
}

struct PooledConnection {
    socket: ProviderSocket,
    pool: Arc<ConnectionPool>,
    _permit: OwnedSemaphorePermit,
}

#[derive(Debug, Clone, Copy)]
struct ReadyLimits {
    max_connections: usize,
}

fn connection_pools() -> &'static StdMutex<HashMap<PoolKey, PoolEntry>> {
    static POOLS: OnceLock<StdMutex<HashMap<PoolKey, PoolEntry>>> = OnceLock::new();
    POOLS.get_or_init(|| StdMutex::new(HashMap::new()))
}

fn next_pool_generation() -> u64 {
    static GENERATION: AtomicU64 = AtomicU64::new(1);
    GENERATION.fetch_add(1, Ordering::Relaxed)
}

fn pool_from_map(
    pools: &mut HashMap<PoolKey, PoolEntry>,
    key: PoolKey,
) -> Option<Arc<ConnectionPool>> {
    let generation = next_pool_generation();
    if let Some(entry) = pools.get_mut(&key) {
        entry.generation = generation;
        return Some(entry.pool.clone());
    }
    if pools.len() >= MAX_CONNECTION_POOLS {
        let eviction = pools
            .iter()
            .filter(|(_, entry)| Arc::strong_count(&entry.pool) == 1)
            .min_by_key(|(_, entry)| entry.generation)
            .map(|(key, _)| key.clone())?;
        pools.remove(&eviction);
    }
    let pool = Arc::new(ConnectionPool {
        state: AsyncMutex::new(PoolState::default()),
    });
    pools.insert(
        key,
        PoolEntry {
            pool: pool.clone(),
            generation,
        },
    );
    Some(pool)
}

fn pool_key(stream_url: &str, authorization: Option<&str>) -> PoolKey {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(authorization.unwrap_or_default().as_bytes());
    PoolKey {
        stream_url: stream_url.to_string(),
        authorization_hash: digest.into(),
    }
}

async fn acquire_connection(
    stream_url: &str,
    authorization: Option<&str>,
) -> Result<PooledConnection, WebSocketRunError> {
    let key = pool_key(stream_url, authorization);
    let pool = {
        let mut pools = connection_pools()
            .lock()
            .map_err(|_| WebSocketRunError::new(WebSocketRunErrorKind::Internal))?;
        pool_from_map(&mut pools, key)
            .ok_or_else(|| WebSocketRunError::new(WebSocketRunErrorKind::Internal))?
    };

    let mut state = pool.state.lock().await;
    if state.semaphore.is_none() {
        let (first, limits) = connect_socket(stream_url, authorization).await?;
        let max_connections = limits.max_connections;
        let mut prewarmed = Vec::with_capacity(max_connections.saturating_sub(1));
        let mut connects = FuturesUnordered::new();
        for _ in 1..max_connections {
            connects.push(connect_socket(stream_url, authorization));
        }
        while let Some(connection) = connects.next().await {
            let (socket, next_limits) = connection?;
            if next_limits.max_connections != max_connections {
                return Err(WebSocketRunError::new(WebSocketRunErrorKind::Protocol));
            }
            prewarmed.push(socket);
        }
        let semaphore = Arc::new(Semaphore::new(max_connections));
        let permit = semaphore
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| WebSocketRunError::new(WebSocketRunErrorKind::Internal))?;
        state.max_connections = max_connections;
        state.semaphore = Some(semaphore);
        state.idle = prewarmed;
        drop(state);
        return Ok(PooledConnection {
            socket: first,
            pool,
            _permit: permit,
        });
    }
    let semaphore = state
        .semaphore
        .as_ref()
        .expect("initialized pool semaphore")
        .clone();
    let expected_max_connections = state.max_connections;
    drop(state);
    let permit = tokio::time::timeout(Duration::from_secs(5), semaphore.acquire_owned())
        .await
        .map_err(|_| WebSocketRunError::new(WebSocketRunErrorKind::Timeout))?
        .map_err(|_| WebSocketRunError::new(WebSocketRunErrorKind::Internal))?;
    let socket = pool.state.lock().await.idle.pop();
    let socket = match socket {
        Some(mut socket) => {
            if idle_socket_is_healthy(&mut socket).await {
                socket
            } else {
                connect_replacement_socket(stream_url, authorization, expected_max_connections)
                    .await?
            }
        }
        None => {
            connect_replacement_socket(stream_url, authorization, expected_max_connections).await?
        }
    };
    Ok(PooledConnection {
        socket,
        pool,
        _permit: permit,
    })
}

async fn connect_replacement_socket(
    stream_url: &str,
    authorization: Option<&str>,
    expected_max_connections: usize,
) -> Result<ProviderSocket, WebSocketRunError> {
    let (socket, limits) = connect_socket(stream_url, authorization).await?;
    if limits.max_connections != expected_max_connections {
        return Err(WebSocketRunError::new(WebSocketRunErrorKind::Protocol));
    }
    Ok(socket)
}

async fn idle_socket_is_healthy(socket: &mut ProviderSocket) -> bool {
    let probe = uuid::Uuid::new_v4().as_bytes()[..8].to_vec();
    if socket
        .send(Message::Ping(probe.clone().into()))
        .await
        .is_err()
    {
        return false;
    }
    let response = tokio::time::timeout(IDLE_HEALTH_TIMEOUT, async {
        loop {
            match socket.next().await {
                Some(Ok(Message::Pong(payload))) if payload.as_ref() == probe.as_slice() => {
                    return true;
                }
                Some(Ok(Message::Ping(payload))) => {
                    if socket.send(Message::Pong(payload)).await.is_err() {
                        return false;
                    }
                }
                Some(Ok(Message::Close(_))) | Some(Err(_)) | None => return false,
                Some(Ok(_)) => return false,
            }
        }
    })
    .await;
    matches!(response, Ok(true))
}

async fn recycle_connection(connection: PooledConnection) {
    let PooledConnection {
        socket,
        pool,
        _permit,
    } = connection;
    pool.state.lock().await.idle.push(socket);
}

async fn connect_socket(
    stream_url: &str,
    authorization: Option<&str>,
) -> Result<(ProviderSocket, ReadyLimits), WebSocketRunError> {
    let mut request = stream_url
        .into_client_request()
        .map_err(|_| WebSocketRunError::new(WebSocketRunErrorKind::Contract))?;
    request.headers_mut().insert(
        header::SEC_WEBSOCKET_PROTOCOL,
        HeaderValue::from_static(SUBPROTOCOL),
    );
    if let Some(authorization) = authorization {
        let mut value = HeaderValue::from_str(authorization)
            .map_err(|_| WebSocketRunError::new(WebSocketRunErrorKind::Authentication))?;
        value.set_sensitive(true);
        request.headers_mut().insert(header::AUTHORIZATION, value);
    }
    let config = WebSocketConfig::default()
        .read_buffer_size(64 * 1_024)
        .write_buffer_size(4 * 1_024)
        .max_write_buffer_size(MAX_SERVER_MESSAGE_BYTES)
        .max_message_size(Some(MAX_SERVER_MESSAGE_BYTES))
        .max_frame_size(Some(MAX_SERVER_MESSAGE_BYTES));
    let connected = tokio::time::timeout(
        Duration::from_secs(5),
        connect_async_with_config(request, Some(config), true),
    )
    .await
    .map_err(|_| WebSocketRunError::new(WebSocketRunErrorKind::Timeout))?
    .map_err(classify_connect_error)?;
    let (mut socket, response) = connected;
    if response
        .headers()
        .get(header::SEC_WEBSOCKET_PROTOCOL)
        .and_then(|value| value.to_str().ok())
        != Some(SUBPROTOCOL)
    {
        return Err(WebSocketRunError::new(WebSocketRunErrorKind::Protocol));
    }
    let ready = tokio::time::timeout(Duration::from_secs(5), socket.next())
        .await
        .map_err(|_| WebSocketRunError::new(WebSocketRunErrorKind::Timeout))?
        .ok_or_else(|| WebSocketRunError::new(WebSocketRunErrorKind::Network))?
        .map_err(|_| WebSocketRunError::new(WebSocketRunErrorKind::Network))?;
    let limits = validate_ready(ready)?;
    Ok((socket, limits))
}

async fn resume_connection(
    stream_url: &str,
    authorization: Option<&str>,
    allocation_id: &str,
    ordered: &OrderedRun,
    acknowledged: &AcknowledgedCheckpoint,
) -> Result<PooledConnection, WebSocketRunError> {
    for _ in 0..2 {
        let mut connection = acquire_connection(stream_url, authorization).await?;
        if send_json(
            &mut connection.socket,
            &ordered.resume(allocation_id, acknowledged),
        )
        .await
        .is_err()
        {
            continue;
        }
        let negotiation_deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        let response = loop {
            let message =
                match tokio::time::timeout_at(negotiation_deadline, connection.socket.next()).await
                {
                    Ok(Some(Ok(message))) => message,
                    _ => break None,
                };
            match message {
                Message::Text(text) => break Some(text),
                Message::Ping(payload) => {
                    if send_message(&mut connection.socket, Message::Pong(payload))
                        .await
                        .is_err()
                    {
                        break None;
                    }
                }
                Message::Pong(_) => {}
                Message::Close(_) | Message::Binary(_) | Message::Frame(_) => break None,
            }
        };
        let Some(response) = response else { continue };
        match validate_resume_negotiation(response.as_str(), ordered.run_id(), acknowledged.seq)
            .map_err(|_| WebSocketRunError::new(WebSocketRunErrorKind::Protocol))?
        {
            ResumeNegotiation::Resumed => return Ok(connection),
        }
    }
    Err(WebSocketRunError::new(WebSocketRunErrorKind::Network))
}

async fn acknowledge_terminal(
    mut connection: PooledConnection,
    stream_url: &str,
    authorization: Option<&str>,
    allocation_id: &str,
    ordered: &OrderedRun,
    acknowledged: &AcknowledgedCheckpoint,
) -> Result<(), WebSocketRunError> {
    if send_ack(&mut connection.socket, ordered).await.is_ok() {
        recycle_connection(connection).await;
        return Ok(());
    }
    drop(connection);
    tokio::time::timeout(Duration::from_secs(5), async {
        let mut resumed = resume_connection(
            stream_url,
            authorization,
            allocation_id,
            ordered,
            acknowledged,
        )
        .await?;
        let _ = send_ack(&mut resumed.socket, ordered).await?;
        Ok::<(), WebSocketRunError>(())
    })
    .await
    .map_err(|_| WebSocketRunError::new(WebSocketRunErrorKind::Timeout))?
}

pub(crate) async fn run(
    context: WebSocketRunContext<'_>,
) -> Result<WebSocketRunResult, WebSocketRunError> {
    validate_stream_url(context.stream_url)?;
    let allocation_id = context
        .allocation_id
        .unwrap_or(context.input.run_id.as_str());
    let deadline = tokio::time::Instant::now() + context.timeout;
    let mut connection = tokio::select! {
        biased;
        _ = context.cancellation.cancelled() => {
            return Ok(WebSocketRunResult::Cancelled { output_started: false });
        }
        _ = tokio::time::sleep_until(deadline) => {
            return Err(WebSocketRunError::new(WebSocketRunErrorKind::Timeout));
        }
        connection = acquire_connection(context.stream_url, context.authorization) => connection?,
    };

    let start = RunStart::new(
        &context.input.run_id,
        allocation_id,
        context.model,
        context.messages,
        context.tools,
        context.reasoning_effort,
        context.max_output_tokens,
        context
            .timeout
            .as_millis()
            .try_into()
            .unwrap_or(3_300_000_u64)
            .min(3_300_000),
    );
    tokio::select! {
        biased;
        _ = context.cancellation.cancelled() => {
            return Ok(WebSocketRunResult::Cancelled { output_started: false });
        }
        _ = tokio::time::sleep_until(deadline) => {
            return Err(WebSocketRunError::new(WebSocketRunErrorKind::Timeout));
        }
        result = send_json(&mut connection.socket, &start) => result?,
    }

    let mut ordered = OrderedRun::new(&context.input.run_id)
        .map_err(|_| WebSocketRunError::new(WebSocketRunErrorKind::Contract))?;
    let mut acknowledged = ordered.checkpoint();
    let mut tool_futures = FuturesUnordered::<ToolFuture<'_>>::new();
    let mut total_tool_calls = 0_usize;
    let mut outstanding_tool_calls = 0_usize;
    let mut output_started = false;
    let mut last_ack_at = tokio::time::Instant::now();
    let mut cancel_deadline = None;

    loop {
        let ack_due = ordered.last_accepted_seq() > acknowledged.seq
            && (ordered.last_accepted_seq().saturating_sub(acknowledged.seq) >= ACK_EVENT_INTERVAL
                || last_ack_at.elapsed() >= ACK_TIME_INTERVAL);
        if ack_due {
            tokio::select! {
                biased;
                _ = context.cancellation.cancelled(), if cancel_deadline.is_none() => {
                    send_json(&mut connection.socket, &RunCancel {
                        message_type: "run.cancel",
                        run_id: &context.input.run_id,
                        reason: "user",
                    }).await.map_err(|error| error.with_output_started(output_started))?;
                    cancel_deadline = Some(tokio::time::Instant::now() + Duration::from_secs(1));
                }
                checkpoint = send_ack(&mut connection.socket, &ordered) => {
                    acknowledged = checkpoint
                        .map_err(|error| error.with_output_started(output_started))?;
                    last_ack_at = tokio::time::Instant::now();
                }
            }
            continue;
        }
        tokio::select! {
            biased;
            _ = context.cancellation.cancelled(), if cancel_deadline.is_none() => {
                send_json(&mut connection.socket, &RunCancel {
                    message_type: "run.cancel",
                    run_id: &context.input.run_id,
                    reason: "user",
                }).await.map_err(|error| error.with_output_started(output_started))?;
                cancel_deadline = Some(tokio::time::Instant::now() + Duration::from_secs(1));
            }
            Some(result) = tool_futures.next(), if !tool_futures.is_empty() && cancel_deadline.is_none() => {
                outstanding_tool_calls = outstanding_tool_calls.saturating_sub(1);
                if result.content.len() > MAX_TOOL_RESULT_BYTES {
                    return Err(WebSocketRunError::new(WebSocketRunErrorKind::Protocol)
                        .with_output_started(output_started));
                }
                send_json(&mut connection.socket, &ToolResultMessage {
                    message_type: "tool.result",
                    run_id: &context.input.run_id,
                    call_id: &result.call_id,
                    tool_call_seq: result.in_reply_to_seq,
                    status: "completed",
                    content: &result.content,
                }).await.map_err(|error| error.with_output_started(output_started))?;
            }
            incoming = connection.socket.next() => {
                let message = match incoming {
                    Some(Ok(Message::Close(_))) | Some(Err(_)) | None => {
                        crate::runtime::event_hub::performance::record_reconnect();
                        drop(connection);
                        connection = tokio::select! {
                            biased;
                            _ = context.cancellation.cancelled() => {
                                return Ok(WebSocketRunResult::Cancelled { output_started });
                            }
                            _ = tokio::time::sleep_until(deadline) => {
                                return Err(WebSocketRunError::new(WebSocketRunErrorKind::Timeout)
                                    .with_output_started(output_started));
                            }
                            resumed = resume_connection(
                                context.stream_url,
                                context.authorization,
                                allocation_id,
                                &ordered,
                                &acknowledged,
                            ) => resumed.map_err(|error| error.with_output_started(output_started))?,
                        };
                        last_ack_at = tokio::time::Instant::now();
                        continue;
                    }
                    Some(Ok(message)) => message,
                };
                let received_at = std::time::Instant::now();
                let accepted = match message {
                    Message::Binary(frame) => ordered.accept_binary(&frame),
                    Message::Text(text) => ordered.accept_text(text.as_str()),
                    Message::Ping(payload) => {
                        send_message(&mut connection.socket, Message::Pong(payload))
                            .await
                            .map_err(|error| error.with_output_started(output_started))?;
                        continue;
                    }
                    Message::Pong(_) => continue,
                    Message::Close(_) => unreachable!("close frames resume before projection"),
                    Message::Frame(_) => return Err(
                        WebSocketRunError::new(WebSocketRunErrorKind::Protocol)
                            .with_output_started(output_started)
                    ),
                };
                let event = match accepted {
                    Ok(event) => event,
                    Err(ProtocolError::SequenceGap) => {
                        crate::runtime::event_hub::performance::record_sequence_gap();
                        crate::runtime::event_hub::performance::record_reconnect();
                        drop(connection);
                        connection = tokio::select! {
                            biased;
                            _ = context.cancellation.cancelled() => {
                                return Ok(WebSocketRunResult::Cancelled { output_started });
                            }
                            _ = tokio::time::sleep_until(deadline) => {
                                return Err(WebSocketRunError::new(WebSocketRunErrorKind::Timeout)
                                    .with_output_started(output_started));
                            }
                            resumed = resume_connection(
                                context.stream_url,
                                context.authorization,
                                allocation_id,
                                &ordered,
                                &acknowledged,
                            ) => resumed.map_err(|error| error.with_output_started(output_started))?,
                        };
                        last_ack_at = tokio::time::Instant::now();
                        continue;
                    }
                    Err(_) => {
                        return Err(WebSocketRunError::new(WebSocketRunErrorKind::Protocol)
                            .with_output_started(output_started));
                    }
                };
                match event {
                    ProviderEvent::Accepted { .. } | ProviderEvent::Duplicate { .. } => {}
                    ProviderEvent::Delta { text, .. } => {
                        if cancel_deadline.is_some() {
                            continue;
                        }
                        if !output_started {
                            if let Some(persistence) = context.output_persistence {
                                persistence.mark_started().map_err(|_| {
                                    WebSocketRunError::new(WebSocketRunErrorKind::Internal)
                                })?;
                            }
                            output_started = true;
                        }
                        context.on_event.send_received(
                            RuntimeEvent::Delta {
                                run_id: context.input.run_id.clone(),
                                text,
                            },
                            received_at,
                        ).map_err(|_| {
                            WebSocketRunError::new(WebSocketRunErrorKind::ClientDisconnected)
                                .with_output_started(true)
                        })?;
                    }
                    ProviderEvent::ToolCall { seq, call_id, name, arguments } => {
                        if cancel_deadline.is_some() {
                            continue;
                        }
                        if total_tool_calls >= 32
                            || outstanding_tool_calls >= 4
                            || !tool_was_offered(context.tools, &name)
                        {
                            return Err(WebSocketRunError::new(WebSocketRunErrorKind::Protocol)
                                .with_output_started(output_started));
                        }
                        acknowledged = send_ack(&mut connection.socket, &ordered)
                            .await
                            .map_err(|error| error.with_output_started(output_started))?;
                        last_ack_at = tokio::time::Instant::now();
                        total_tool_calls += 1;
                        outstanding_tool_calls += 1;
                        let call = AgentToolCall {
                            id: call_id.clone(),
                            name,
                            arguments: serde_json::to_string(&arguments)
                                .map_err(|_| WebSocketRunError::new(WebSocketRunErrorKind::Protocol)
                                    .with_output_started(output_started))?,
                        };
                        let persistence = context.output_persistence;
                        let input = context.input;
                        let timeout = context.tool_timeout;
                        tool_futures.push(Box::pin(async move {
                            let content = execute_agent_tool(persistence, input, &call, timeout).await;
                            ToolResult { call_id, in_reply_to_seq: seq, content }
                        }));
                    }
                    ProviderEvent::Completed { finish_reason, .. } => {
                        let cancellation_won = cancel_deadline.is_some();
                        if !cancellation_won && outstanding_tool_calls != 0 {
                            return Err(WebSocketRunError::new(WebSocketRunErrorKind::Protocol)
                                .with_output_started(output_started));
                        }
                        let result = if cancellation_won {
                            WebSocketRunResult::Cancelled { output_started }
                        } else {
                            match finish_reason {
                                FinishReason::Stop => WebSocketRunResult::Completed(ordered.content().to_string()),
                                FinishReason::Length => WebSocketRunResult::Length(ordered.content().to_string()),
                            }
                        };
                        acknowledge_terminal(
                            connection,
                            context.stream_url,
                            context.authorization,
                            allocation_id,
                            &ordered,
                            &acknowledged,
                        ).await
                            .map_err(|error| error.with_output_started(output_started))?;
                        return Ok(result);
                    }
                    ProviderEvent::Failed { code, output_started: provider_output_started, .. } => {
                        let effective_output_started = output_started || provider_output_started;
                        let result = if cancel_deadline.is_some() {
                            WebSocketRunResult::Cancelled {
                                output_started,
                            }
                        } else {
                            WebSocketRunResult::Failed {
                                code,
                                output_started: effective_output_started,
                            }
                        };
                        acknowledge_terminal(
                            connection,
                            context.stream_url,
                            context.authorization,
                            allocation_id,
                            &ordered,
                            &acknowledged,
                        ).await
                            .map_err(|error| error.with_output_started(effective_output_started))?;
                        return Ok(result);
                    }
                    ProviderEvent::Cancelled { output_started: provider_output_started, .. } => {
                        let effective_output_started = output_started || provider_output_started;
                        acknowledge_terminal(
                            connection,
                            context.stream_url,
                            context.authorization,
                            allocation_id,
                            &ordered,
                            &acknowledged,
                        ).await
                            .map_err(|error| error.with_output_started(effective_output_started))?;
                        let result = WebSocketRunResult::Cancelled {
                            output_started: effective_output_started,
                        };
                        return Ok(result);
                    }
                }
            }
            _ = tokio::time::sleep_until(deadline) => return Err(
                WebSocketRunError::new(WebSocketRunErrorKind::Timeout)
                    .with_output_started(output_started)
            ),
            _ = async {
                if let Some(deadline) = cancel_deadline {
                    tokio::time::sleep_until(deadline).await;
                }
            }, if cancel_deadline.is_some() => {
                return Ok(WebSocketRunResult::Cancelled { output_started });
            }
            _ = tokio::time::sleep(ACK_TIME_INTERVAL), if ordered.last_accepted_seq() > acknowledged.seq => {}
        }
    }
}

fn validate_stream_url(value: &str) -> Result<(), WebSocketRunError> {
    let url = url::Url::parse(value)
        .map_err(|_| WebSocketRunError::new(WebSocketRunErrorKind::Contract))?;
    if !matches!(url.scheme(), "ws" | "wss")
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || (url.scheme() == "ws" && !crate::providers::dynamic_lan::url_is_local(&url))
    {
        return Err(WebSocketRunError::new(WebSocketRunErrorKind::Contract));
    }
    Ok(())
}

fn validate_ready(message: Message) -> Result<ReadyLimits, WebSocketRunError> {
    let Message::Text(text) = message else {
        return Err(WebSocketRunError::new(WebSocketRunErrorKind::Protocol));
    };
    reject_duplicate_keys(text.as_str())
        .map_err(|_| WebSocketRunError::new(WebSocketRunErrorKind::Protocol))?;
    let ready: ConnectionReady = serde_json::from_str(text.as_str())
        .map_err(|_| WebSocketRunError::new(WebSocketRunErrorKind::Protocol))?;
    if ready.message_type != "connection.ready"
        || ready.protocol != SUBPROTOCOL
        || !valid_connection_identifier(&ready.connection_id)
        || !matches!(ready.upstream_transport.as_str(), "native" | "websocket")
        || ready.limits.max_active_runs_per_connection != 1
        || !(1..=8).contains(&ready.limits.max_connections)
        || ready.limits.max_concurrent_runs != ready.limits.max_connections
        || ready.limits.max_unacked_events != 64
        || ready.limits.max_unacked_bytes != MAX_SERVER_MESSAGE_BYTES
        || ready.limits.resume_window_ms < 120_000
        || ready.limits.heartbeat_interval_ms != 15_000
    {
        return Err(WebSocketRunError::new(WebSocketRunErrorKind::Protocol));
    }
    Ok(ReadyLimits {
        max_connections: ready.limits.max_connections,
    })
}

fn valid_connection_identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 160
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
}

async fn send_ack<S>(
    socket: &mut S,
    run: &OrderedRun,
) -> Result<AcknowledgedCheckpoint, WebSocketRunError>
where
    S: futures_util::Sink<Message> + Unpin,
{
    let checkpoint = run.checkpoint();
    send_json(socket, &run.ack(&checkpoint)).await?;
    Ok(checkpoint)
}

async fn send_json<S, T>(socket: &mut S, value: &T) -> Result<(), WebSocketRunError>
where
    S: futures_util::Sink<Message> + Unpin,
    T: Serialize,
{
    let text = serde_json::to_string(value)
        .map_err(|_| WebSocketRunError::new(WebSocketRunErrorKind::Internal))?;
    send_message(socket, Message::Text(text.into())).await
}

async fn send_message<S>(socket: &mut S, message: Message) -> Result<(), WebSocketRunError>
where
    S: futures_util::Sink<Message> + Unpin,
{
    tokio::time::timeout(WRITE_TIMEOUT, socket.send(message))
        .await
        .map_err(|_| WebSocketRunError::new(WebSocketRunErrorKind::Timeout))?
        .map_err(|_| WebSocketRunError::new(WebSocketRunErrorKind::Network))
}

fn classify_connect_error(error: tokio_tungstenite::tungstenite::Error) -> WebSocketRunError {
    match error {
        tokio_tungstenite::tungstenite::Error::Http(response)
            if matches!(response.status().as_u16(), 401 | 403) =>
        {
            WebSocketRunError::new(WebSocketRunErrorKind::Authentication)
        }
        tokio_tungstenite::tungstenite::Error::Url(_)
        | tokio_tungstenite::tungstenite::Error::Http(_) => {
            WebSocketRunError::new(WebSocketRunErrorKind::Contract)
        }
        _ => WebSocketRunError::new(WebSocketRunErrorKind::Network),
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ConnectionReady {
    #[serde(rename = "type")]
    message_type: String,
    protocol: String,
    connection_id: String,
    upstream_transport: String,
    limits: ConnectionLimits,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ConnectionLimits {
    max_concurrent_runs: usize,
    max_connections: usize,
    max_active_runs_per_connection: usize,
    max_unacked_events: usize,
    max_unacked_bytes: usize,
    resume_window_ms: u64,
    heartbeat_interval_ms: u64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct RunCancel<'a> {
    #[serde(rename = "type")]
    message_type: &'static str,
    run_id: &'a str,
    reason: &'static str,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ToolResultMessage<'a> {
    #[serde(rename = "type")]
    message_type: &'static str,
    run_id: &'a str,
    call_id: &'a str,
    tool_call_seq: u64,
    status: &'static str,
    content: &'a str,
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::Digest as _;
    use tokio::net::TcpListener;
    use tokio_tungstenite::{
        accept_hdr_async,
        tungstenite::handshake::server::{Request, Response},
    };

    fn ready_message(max_connections: usize) -> Message {
        Message::Text(
            serde_json::json!({
                "type": "connection.ready",
                "protocol": SUBPROTOCOL,
                "connectionId": "connection_test",
                "upstreamTransport": "native",
                "limits": {
                    "maxConcurrentRuns": max_connections,
                    "maxConnections": max_connections,
                    "maxActiveRunsPerConnection": 1,
                    "maxUnackedEvents": 64,
                    "maxUnackedBytes": MAX_SERVER_MESSAGE_BYTES,
                    "resumeWindowMs": 120_000,
                    "heartbeatIntervalMs": 15_000
                }
            })
            .to_string()
            .into(),
        )
    }

    #[test]
    fn connection_ready_requires_exact_limits_and_rejects_duplicate_keys() {
        assert_eq!(
            validate_ready(ready_message(2))
                .expect("valid ready")
                .max_connections,
            2
        );
        let mismatch = Message::Text(
            r#"{"type":"connection.ready","protocol":"saaa.llm-stream.v1","connectionId":"connection_test","upstreamTransport":"native","limits":{"maxConcurrentRuns":2,"maxConnections":1,"maxActiveRunsPerConnection":1,"maxUnackedEvents":64,"maxUnackedBytes":524288,"resumeWindowMs":120000,"heartbeatIntervalMs":15000}}"#.into(),
        );
        assert_eq!(
            validate_ready(mismatch).unwrap_err().kind,
            WebSocketRunErrorKind::Protocol
        );
        let duplicate = Message::Text(
            r#"{"type":"connection.ready","protocol":"saaa.llm-stream.v1","connectionId":"a","connectionId":"b","upstreamTransport":"native","limits":{"maxConcurrentRuns":1,"maxConnections":1,"maxActiveRunsPerConnection":1,"maxUnackedEvents":64,"maxUnackedBytes":524288,"resumeWindowMs":120000,"heartbeatIntervalMs":15000}}"#.into(),
        );
        assert_eq!(
            validate_ready(duplicate).unwrap_err().kind,
            WebSocketRunErrorKind::Protocol
        );
    }

    #[test]
    fn plaintext_websocket_is_limited_to_local_network_hosts() {
        for value in [
            "ws://localhost:9810/v1/llm/stream",
            "ws://192.168.0.130:9810/v1/llm/stream",
            "ws://10.0.0.42:9810/v1/llm/stream",
            "ws://provider.local:9810/v1/llm/stream",
            "ws://dynamic-lan:9810/v1/llm/stream",
            "ws://[fd00::42]:9810/v1/llm/stream",
        ] {
            assert!(validate_stream_url(value).is_ok(), "{value}");
        }
        for value in [
            "ws://8.8.8.8/v1/llm/stream",
            "ws://example.com/v1/llm/stream",
            "ws://user@example.com/v1/llm/stream",
            "ws://192.168.0.130/v1/llm/stream?token=secret",
        ] {
            assert_eq!(
                validate_stream_url(value).unwrap_err().kind,
                WebSocketRunErrorKind::Contract,
                "{value}"
            );
        }
        assert!(validate_stream_url("wss://example.com/v1/llm/stream").is_ok());
    }

    #[test]
    fn idle_connection_pool_cache_evicts_the_least_recently_used_endpoint() {
        let mut pools = HashMap::new();
        for index in 0..=MAX_CONNECTION_POOLS {
            let key = pool_key(
                &format!("ws://127.0.0.1:{}/v1/llm/stream", 20_000 + index),
                None,
            );
            let _ = pool_from_map(&mut pools, key).expect("idle pool remains evictable");
        }
        assert_eq!(pools.len(), MAX_CONNECTION_POOLS);
        assert!(!pools.contains_key(&pool_key("ws://127.0.0.1:20000/v1/llm/stream", None)));
    }

    #[test]
    fn active_connection_pool_cache_fails_closed_at_the_global_bound() {
        let mut pools = HashMap::new();
        let mut active = Vec::new();
        for index in 0..MAX_CONNECTION_POOLS {
            let key = pool_key(
                &format!("ws://127.0.0.1:{}/v1/llm/stream", 30_000 + index),
                None,
            );
            active.push(pool_from_map(&mut pools, key).expect("pool fits within bound"));
        }
        let overflow = pool_key("ws://127.0.0.1:40000/v1/llm/stream", None);
        assert!(pool_from_map(&mut pools, overflow).is_none());
        assert_eq!(pools.len(), MAX_CONNECTION_POOLS);
        assert_eq!(active.len(), MAX_CONNECTION_POOLS);
    }

    fn binary_delta(seq: u64, text: &str) -> Message {
        let mut frame = Vec::with_capacity(16 + text.len());
        frame.extend_from_slice(b"SAD1");
        frame.extend_from_slice(&[1, 0]);
        frame.extend_from_slice(&16_u16.to_be_bytes());
        frame.extend_from_slice(&seq.to_be_bytes());
        frame.extend_from_slice(text.as_bytes());
        Message::Binary(frame.into())
    }

    async fn accept_test_socket(listener: &TcpListener) -> WebSocketStream<TcpStream> {
        let (stream, _) = listener.accept().await.unwrap();
        let mut socket = accept_hdr_async(stream, |request: &Request, mut response: Response| {
            assert_eq!(
                request
                    .headers()
                    .get(header::SEC_WEBSOCKET_PROTOCOL)
                    .and_then(|value| value.to_str().ok()),
                Some(SUBPROTOCOL)
            );
            response.headers_mut().insert(
                header::SEC_WEBSOCKET_PROTOCOL,
                HeaderValue::from_static(SUBPROTOCOL),
            );
            Ok(response)
        })
        .await
        .unwrap();
        socket.send(ready_message(1)).await.unwrap();
        socket
    }

    async fn next_text(socket: &mut WebSocketStream<TcpStream>) -> String {
        loop {
            match socket.next().await.unwrap().unwrap() {
                Message::Text(text) => return text.to_string(),
                Message::Ping(payload) => socket.send(Message::Pong(payload)).await.unwrap(),
                message => panic!("unexpected fixture message: {message:?}"),
            }
        }
    }

    fn test_input(run_id: &str) -> StartTurnInput {
        StartTurnInput {
            run_id: run_id.to_string(),
            conversation_id: "conversation_websocket_test".to_string(),
            content: "fixture".to_string(),
            workspace_path: None,
            retry_input_message_id: None,
            source_id: None,
            input_origin: "text".to_string(),
            presentation_mode: "visual".to_string(),
        }
    }

    #[tokio::test]
    async fn observed_delta_prevents_fallback_when_failed_claims_output_not_started() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let mut socket = accept_test_socket(&listener).await;
            let start: Value = serde_json::from_str(&next_text(&mut socket).await).unwrap();
            let run_id = start["runId"].as_str().unwrap();
            socket
                .send(Message::Text(
                    serde_json::json!({
                        "type": "run.accepted",
                        "runId": run_id,
                        "seq": 1
                    })
                    .to_string()
                    .into(),
                ))
                .await
                .unwrap();
            socket.send(binary_delta(2, "visible")).await.unwrap();
            socket
                .send(Message::Text(
                    serde_json::json!({
                        "type": "response.failed",
                        "runId": run_id,
                        "seq": 3,
                        "contentBytes": 7,
                        "contentSha256": format!("{:x}", sha2::Sha256::digest(b"visible")),
                        "error": {
                            "code": "provider-error",
                            "message": "Provider failed.",
                            "retryable": true
                        }
                    })
                    .to_string()
                    .into(),
                ))
                .await
                .unwrap();
            let ack: Value = serde_json::from_str(&next_text(&mut socket).await).unwrap();
            assert_eq!(ack["ackSeq"], 3);
        });

        let input = test_input("run_failed_output_fixture");
        let messages = [serde_json::json!({ "role": "user", "content": "fixture" })];
        let sink = tauri::ipc::Channel::new(|_| Ok(()));
        let result = run(WebSocketRunContext {
            stream_url: &format!("ws://{address}/v1/llm/stream"),
            authorization: None,
            allocation_id: Some("alloc_test"),
            model: "local",
            messages: &messages,
            tools: &[],
            reasoning_effort: "medium",
            max_output_tokens: 64,
            tool_timeout: Duration::from_secs(1),
            timeout: Duration::from_secs(5),
            input: &input,
            on_event: &sink,
            cancellation: Arc::new(RunCancellation::default()),
            output_persistence: None,
        })
        .await
        .unwrap();
        assert_eq!(
            result,
            WebSocketRunResult::Failed {
                code: "provider-error".to_string(),
                output_started: true,
            }
        );
        server.await.unwrap();
    }

    #[tokio::test]
    async fn cancellation_drops_late_delta_after_run_cancel() {
        use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let mut socket = accept_test_socket(&listener).await;
            let start: Value = serde_json::from_str(&next_text(&mut socket).await).unwrap();
            let run_id = start["runId"].as_str().unwrap();
            socket
                .send(Message::Text(
                    serde_json::json!({
                        "type": "run.accepted",
                        "runId": run_id,
                        "seq": 1
                    })
                    .to_string()
                    .into(),
                ))
                .await
                .unwrap();
            loop {
                let message: Value = serde_json::from_str(&next_text(&mut socket).await).unwrap();
                if message["type"] == "run.cancel" {
                    break;
                }
            }
            socket.send(binary_delta(2, "late")).await.unwrap();
            socket
                .send(Message::Text(
                    serde_json::json!({
                        "type": "response.cancelled",
                        "runId": run_id,
                        "seq": 3,
                        "contentBytes": 4,
                        "contentSha256": format!("{:x}", sha2::Sha256::digest(b"late"))
                    })
                    .to_string()
                    .into(),
                ))
                .await
                .unwrap();
            let ack: Value = serde_json::from_str(&next_text(&mut socket).await).unwrap();
            assert_eq!(ack["ackSeq"], 3);
        });

        let cancellation = Arc::new(RunCancellation::default());
        let trigger = cancellation.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(20)).await;
            trigger.cancel();
        });
        let dispatched = Arc::new(AtomicUsize::new(0));
        let observed = dispatched.clone();
        let sink = tauri::ipc::Channel::new(move |_| {
            observed.fetch_add(1, AtomicOrdering::Relaxed);
            Ok(())
        });
        let input = test_input("run_cancel_output_fixture");
        let messages = [serde_json::json!({ "role": "user", "content": "fixture" })];
        let result = run(WebSocketRunContext {
            stream_url: &format!("ws://{address}/v1/llm/stream"),
            authorization: None,
            allocation_id: Some("alloc_test"),
            model: "local",
            messages: &messages,
            tools: &[],
            reasoning_effort: "medium",
            max_output_tokens: 64,
            tool_timeout: Duration::from_secs(1),
            timeout: Duration::from_secs(5),
            input: &input,
            on_event: &sink,
            cancellation,
            output_persistence: None,
        })
        .await
        .unwrap();
        assert_eq!(
            result,
            WebSocketRunResult::Cancelled {
                output_started: true,
            }
        );
        assert_eq!(dispatched.load(AtomicOrdering::Relaxed), 0);
        server.await.unwrap();
    }

    #[tokio::test]
    async fn sequence_gap_resumes_from_the_last_acknowledged_checkpoint() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let mut first = accept_test_socket(&listener).await;
            let start: Value = serde_json::from_str(&next_text(&mut first).await).unwrap();
            let run_id = start["runId"].as_str().unwrap().to_string();
            first
                .send(Message::Text(
                    serde_json::json!({
                        "type": "run.accepted",
                        "runId": run_id,
                        "seq": 1
                    })
                    .to_string()
                    .into(),
                ))
                .await
                .unwrap();
            first.send(binary_delta(3, "out-of-order")).await.unwrap();
            drop(first);

            let mut resumed = accept_test_socket(&listener).await;
            let resume: Value = serde_json::from_str(&next_text(&mut resumed).await).unwrap();
            assert_eq!(resume["type"], "run.resume");
            assert_eq!(resume["runId"], run_id);
            assert_eq!(resume["ackSeq"], 0);
            resumed
                .send(Message::Text(
                    serde_json::json!({
                        "type": "run.resumed",
                        "runId": run_id,
                        "ackSeq": 0
                    })
                    .to_string()
                    .into(),
                ))
                .await
                .unwrap();
            resumed
                .send(Message::Text(
                    serde_json::json!({
                        "type": "run.accepted",
                        "runId": run_id,
                        "seq": 1
                    })
                    .to_string()
                    .into(),
                ))
                .await
                .unwrap();
            resumed.send(binary_delta(2, "recovered")).await.unwrap();
            let hash = {
                use sha2::{Digest, Sha256};
                format!("{:x}", Sha256::digest(b"recovered"))
            };
            resumed
                .send(Message::Text(
                    serde_json::json!({
                        "type": "response.completed",
                        "runId": run_id,
                        "seq": 3,
                        "contentBytes": 9,
                        "contentSha256": hash,
                        "finishReason": "stop",
                        "usage": null
                    })
                    .to_string()
                    .into(),
                ))
                .await
                .unwrap();
            let ack: Value = serde_json::from_str(&next_text(&mut resumed).await).unwrap();
            assert_eq!(ack["ackSeq"], 3);
        });

        let input = test_input("run_gap_fixture");
        let messages = [serde_json::json!({ "role": "user", "content": "fixture" })];
        let sink = tauri::ipc::Channel::new(|_| Ok(()));
        let result = run(WebSocketRunContext {
            stream_url: &format!("ws://{address}/v1/llm/stream"),
            authorization: None,
            allocation_id: Some("alloc_test"),
            model: "local",
            messages: &messages,
            tools: &[],
            reasoning_effort: "medium",
            max_output_tokens: 64,
            tool_timeout: Duration::from_secs(1),
            timeout: Duration::from_secs(5),
            input: &input,
            on_event: &sink,
            cancellation: Arc::new(RunCancellation::default()),
            output_persistence: None,
        })
        .await
        .unwrap();
        assert_eq!(
            result,
            WebSocketRunResult::Completed("recovered".to_string())
        );
        server.await.unwrap();
    }

    #[tokio::test]
    async fn one_healthy_connection_serves_one_hundred_sequential_turns() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut socket =
                accept_hdr_async(stream, |request: &Request, mut response: Response| {
                    assert_eq!(
                        request
                            .headers()
                            .get(header::SEC_WEBSOCKET_PROTOCOL)
                            .and_then(|value| value.to_str().ok()),
                        Some(SUBPROTOCOL)
                    );
                    response.headers_mut().insert(
                        header::SEC_WEBSOCKET_PROTOCOL,
                        HeaderValue::from_static(SUBPROTOCOL),
                    );
                    Ok(response)
                })
                .await
                .unwrap();
            socket.send(ready_message(1)).await.unwrap();

            for turn in 0..100 {
                let start = loop {
                    match socket.next().await.unwrap().unwrap() {
                        Message::Ping(_) => socket.flush().await.unwrap(),
                        Message::Text(text) => break text,
                        message => panic!("unexpected pre-turn message: {message:?}"),
                    }
                };
                let start: Value = serde_json::from_str(start.as_str()).unwrap();
                assert_eq!(start["type"], "run.start");
                let run_id = start["runId"].as_str().unwrap();
                assert_eq!(run_id, format!("run_pool_{turn}"));
                socket
                    .send(Message::Text(
                        serde_json::json!({
                            "type": "run.accepted",
                            "runId": run_id,
                            "seq": 1
                        })
                        .to_string()
                        .into(),
                    ))
                    .await
                    .unwrap();
                socket.send(binary_delta(2, "ok")).await.unwrap();
                let hash = {
                    use sha2::{Digest, Sha256};
                    format!("{:x}", Sha256::digest(b"ok"))
                };
                socket
                    .send(Message::Text(
                        serde_json::json!({
                            "type": "response.completed",
                            "runId": run_id,
                            "seq": 3,
                            "contentBytes": 2,
                            "contentSha256": hash,
                            "finishReason": "stop",
                            "usage": null
                        })
                        .to_string()
                        .into(),
                    ))
                    .await
                    .unwrap();
                let ack = socket.next().await.unwrap().unwrap();
                let Message::Text(ack) = ack else {
                    panic!("expected terminal ack")
                };
                let ack: Value = serde_json::from_str(ack.as_str()).unwrap();
                assert_eq!(ack["type"], "run.ack");
                assert_eq!(ack["ackSeq"], 3);
            }
        });

        let stream_url = format!("ws://{address}/v1/llm/stream");
        let sink = tauri::ipc::Channel::new(|_| Ok(()));
        for turn in 0..100 {
            let input = StartTurnInput {
                run_id: format!("run_pool_{turn}"),
                conversation_id: "conversation_pool".to_string(),
                content: "fixture".to_string(),
                workspace_path: None,
                retry_input_message_id: None,
                source_id: None,
                input_origin: "text".to_string(),
                presentation_mode: "visual".to_string(),
            };
            let messages = [serde_json::json!({ "role": "user", "content": "fixture" })];
            let result = run(WebSocketRunContext {
                stream_url: &stream_url,
                authorization: None,
                allocation_id: Some("alloc_test"),
                model: "local",
                messages: &messages,
                tools: &[],
                reasoning_effort: "medium",
                max_output_tokens: 64,
                tool_timeout: Duration::from_secs(1),
                timeout: Duration::from_secs(5),
                input: &input,
                on_event: &sink,
                cancellation: Arc::new(RunCancellation::default()),
                output_persistence: None,
            })
            .await
            .unwrap();
            assert_eq!(result, WebSocketRunResult::Completed("ok".to_string()));
        }
        server.await.unwrap();
    }
}
