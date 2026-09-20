//! D5: publishes SAAA's three fixed selection entry points as a local MCP server.
//!
//! The server reuses the conversation gateway, ACL and reference store; it never builds a second
//! ledger or a second application state. It is loopback-only, bearer-authenticated with a
//! constant-time comparison, JSON-only on the wire, and does not treat a TCP disconnect as a
//! cancellation. `start` can be called from tests and returns a handle that stops the listener.

#![allow(private_interfaces)]

pub mod calls;
pub mod cleanup;
pub mod config;
pub mod context;
pub mod http;
pub mod protocol;
pub mod router;
pub mod sessions;

pub use config::{MCP_SERVER_BIND_ADDRESS, MCP_SERVER_ENDPOINT};

mod tests;

use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use crate::persistence::SqliteWriter;
use crate::tool_selection::service::{self, ToolSelectionService};

/// Protocol revision this server speaks. Version negotiation always answers with this value.
pub const MCP_SERVER_PROTOCOL_VERSION: &str = super::mcp::MCP_PROTOCOL_VERSION;
pub const MCP_SERVER_NAME: &str = "saaa-tool-gateway";
pub const MCP_SERVER_VERSION: &str = env!("CARGO_PKG_VERSION");

pub const SESSION_HEADER: &str = "mcp-session-id";
pub const PROTOCOL_HEADER: &str = "mcp-protocol-version";
pub const BODY_MAX_BYTES: usize = 64 * 1024;
pub const BODY_READ_TIMEOUT: Duration = Duration::from_secs(10);
pub const SHUTDOWN_DRAIN_TIMEOUT: Duration = Duration::from_secs(5);
pub const SHUTDOWN_GRACE_TIMEOUT: Duration = Duration::from_secs(10);

/// Shared server state. Built once per listener; dropping it does not close the database or stop
/// the D4 manager, which are owned elsewhere.
pub struct ServerInner {
    pub service: Arc<ToolSelectionService>,
    pub writer: Arc<SqliteWriter>,
    pub token: String,
    /// Exact host header this listener accepts, e.g. `127.0.0.1:43127`.
    pub host: String,
    pub principal: String,
    pub project_id: Option<String>,
    pub sessions: sessions::SessionRegistry,
    pub shutting_down: AtomicBool,
    /// Whole-call deadline in milliseconds. Fixed at 30s in production; the acceptance tests lower
    /// it to prove the deadline path without a 30-second wall-clock wait.
    pub call_deadline_ms: AtomicU64,
}

impl ServerInner {
    pub fn is_shutting_down(&self) -> bool {
        self.shutting_down.load(Ordering::SeqCst)
    }

    /// Releases a session's scenario cache, references and continuation results. Audit history is
    /// never deleted here. A session that never reached Ready also loses its empty conversation
    /// row so abandoned initializations cannot accumulate.
    pub fn discard_session_scope(&self, session: &sessions::Session) {
        self.service.discard_run_scope(session.run_id());
        if !session.was_ready() {
            cleanup::delete_empty_conversation(&self.writer, session);
        }
    }
}

pub struct ServerHandle {
    port: u16,
    inner: Arc<ServerInner>,
    shutdown: Option<tokio::sync::oneshot::Sender<()>>,
    task: tokio::task::JoinHandle<()>,
}

impl ServerHandle {
    pub fn port(&self) -> u16 {
        self.port
    }

    pub fn is_shutting_down(&self) -> bool {
        self.inner.is_shutting_down()
    }

    /// Stops the listener, cancels in-flight calls, waits briefly for their management tasks to
    /// settle, then releases session scope state. Audit history is never deleted.
    pub async fn shutdown(mut self) {
        self.inner.shutting_down.store(true, Ordering::SeqCst);
        let _ = self.shutdown.take().map(|sender| sender.send(()));
        let closing = self.inner.sessions.close_all();
        let deadline = tokio::time::Instant::now() + SHUTDOWN_DRAIN_TIMEOUT;
        while self.inner.sessions.global_in_flight() > 0 && tokio::time::Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        // Release session scope only after in-flight tasks settle; audit history is kept.
        for session in &closing {
            self.inner.discard_session_scope(session);
        }
        let _ = tokio::time::timeout(SHUTDOWN_GRACE_TIMEOUT, self.task).await;
    }
}

/// Starts the listener and returns its handle. The caller must already hold the service and the
/// single writer; nothing here opens a database or a backend.
pub async fn start(
    service: Arc<ToolSelectionService>,
    writer: Arc<SqliteWriter>,
    config: config::McpServerConfig,
) -> Result<ServerHandle, &'static str> {
    if !config.enabled {
        return Err("mcp-server-disabled");
    }
    let token = config::load_token(&config)?;
    if let Some(project_id) = config.project_id.as_deref() {
        if !context::project_exists(&writer, project_id) {
            return Err("mcp-server-project-unknown");
        }
    }
    let principal = service::ensure_principal(&writer).map_err(|_| "mcp-server-principal")?;

    let address = SocketAddr::from(([127, 0, 0, 1], config.port));
    let listener = tokio::net::TcpListener::bind(address)
        .await
        .map_err(|_| "mcp-server-bind-failed")?;
    let port = listener
        .local_addr()
        .map_err(|_| "mcp-server-bind-failed")?
        .port();

    let inner = Arc::new(ServerInner {
        service,
        writer,
        token,
        host: format!("{}:{port}", config::MCP_SERVER_BIND_ADDRESS),
        principal,
        project_id: config.project_id.clone(),
        sessions: sessions::SessionRegistry::new(),
        shutting_down: AtomicBool::new(false),
        call_deadline_ms: AtomicU64::new(calls::CALL_DEADLINE.as_millis() as u64),
    });
    let app = router::router(inner.clone());
    let (sender, receiver) = tokio::sync::oneshot::channel::<()>();
    let server = axum::serve(listener, app).with_graceful_shutdown(async move {
        let _ = receiver.await;
    });
    let task = tokio::spawn(async move {
        let _ = server.await;
    });

    Ok(ServerHandle {
        port,
        inner,
        shutdown: Some(sender),
        task,
    })
}

/// Application entry point wiring: read the environment configuration and start
/// the listener. A missing/disabled configuration returns `None`; a bind or
/// configuration failure disables only this listener and never the conversation
/// path or the D4 client connections.
pub async fn start_from_environment(
    service: Arc<ToolSelectionService>,
    writer: Arc<SqliteWriter>,
) -> Option<ServerHandle> {
    let config = match config::from_environment() {
        Ok(Some(config)) => config,
        Ok(None) => return None,
        Err(diagnostic) => {
            eprintln!("mcp server configuration invalid: {diagnostic}");
            return None;
        }
    };
    match start(service.clone(), writer, config).await {
        Ok(handle) => {
            if let Some(manager) = service.mcp_manager() {
                let endpoint = format!("loopback:{}/mcp", handle.port());
                manager.set_self_endpoint(Some(endpoint)).await;
            }
            Some(handle)
        }
        Err(diagnostic) => {
            eprintln!("mcp server disabled: {diagnostic}");
            None
        }
    }
}

/// Shuts down a handle held in a mutex slot, if any. The app shutdown path calls this so the
/// listener stops without blocking the synchronous teardown; the spawned task awaits the drain.
pub fn shutdown_slot(slot: &std::sync::Mutex<Option<ServerHandle>>) {
    if let Ok(mut guard) = slot.lock() {
        if let Some(server) = guard.take() {
            tauri::async_runtime::spawn(async move { server.shutdown().await });
        }
    }
}
