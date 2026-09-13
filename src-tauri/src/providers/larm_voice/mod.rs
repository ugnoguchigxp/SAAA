//! Desktop-only owner: ephemeral credentials stay in Rust, shared with the embedded MCP service.
use saaa_larm_session::Session;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, OnceLock,
};
use tokio::sync::{watch, Mutex, OnceCell};
pub(crate) mod audio;
mod decision;

pub(crate) struct Ready {
    pub session: Arc<Session>,
    pub client: Arc<crate::providers::reasoning_mcp::Client>,
    server_stop: watch::Sender<bool>,
}
struct Owner {
    id: String,
    conversation: String,
    cancel: watch::Sender<bool>,
    ready: OnceCell<Result<Arc<Ready>, StartupError>>,
}
static OWNER: Mutex<Option<Arc<Owner>>> = Mutex::const_new(None);
static SHUTTING_DOWN: AtomicBool = AtomicBool::new(false);
static MODE: OnceLock<bool> = OnceLock::new();
pub(crate) fn enabled() -> bool {
    *MODE.get_or_init(|| std::env::var("SAAA_CONVERSATION_REASONING_MODE").as_deref() == Ok("larm"))
}
#[tauri::command]
pub(crate) async fn begin_larm_voice_session(
    owner_id: String,
    conversation_id: String,
) -> Result<(), String> {
    if !enabled() {
        return Ok(());
    }
    crate::validate_identifier(&owner_id, "voice owner")?;
    crate::validate_identifier(&conversation_id, "conversation id")?;
    let mut current = OWNER.lock().await;
    if SHUTTING_DOWN.load(Ordering::Acquire) {
        return Err("LARM voice runtime is shutting down".into());
    }
    if let Some(previous) = current.as_ref() {
        if previous.id != owner_id || previous.conversation != conversation_id {
            previous.cancel.send_replace(true);
            match previous.ready.get_or_init(|| initialize(previous)).await {
                Ok(ready) => ready.close().await?,
                Err(error) => error.release().await?,
            }
            *current = None;
        }
    }
    let owner = current
        .get_or_insert_with(|| {
            let (cancel, _) = watch::channel(false);
            Arc::new(Owner {
                id: owner_id,
                conversation: conversation_id,
                cancel,
                ready: OnceCell::new(),
            })
        })
        .clone();
    drop(current);
    // Detached initializer completes cleanup even if the invoking frontend disappears.
    let task = tokio::spawn(async move {
        let result = owner.ready.get_or_init(|| initialize(&owner)).await;
        result.as_ref().map(|_| ()).map_err(|e| e.message.clone())
    });
    task.await
        .map_err(|_| "LARM session startup task failed".to_string())?
}
async fn initialize(owner: &Owner) -> Result<Arc<Ready>, StartupError> {
    let base = std::env::var("SAAA_LARM_CONTROL_URL")
        .unwrap_or_else(|_| "http://gnosis.local:9810".into());
    let session = Session::connect(&base, owner.cancel.subscribe())
        .await
        .map_err(|error| StartupError {
            message: error.to_string(),
            cleanup: error.cleanup,
        })?;
    let result = async {
        let token = uuid::Uuid::new_v4().to_string();
        let provider = saaa_reasoning_mcp::provider::Provider::from_larm(session.clone())?;
        let service = saaa_reasoning_mcp::Service::new(provider, token.clone())?;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .map_err(|_| "MCP bind failed")?;
        let address = listener
            .local_addr()
            .map_err(|_| "MCP address unavailable")?;
        let client = Arc::new(crate::providers::reasoning_mcp::Client::new(
            &format!("http://{address}/mcp"),
            token,
        )?);
        let (server_stop, mut shutdown) = watch::channel(false);
        tokio::spawn(async move {
            let _ = axum::serve(listener, service.router())
                .with_graceful_shutdown(async move {
                    while !*shutdown.borrow_and_update() {
                        if shutdown.changed().await.is_err() {
                            break;
                        }
                    }
                })
                .await;
        });
        Ok::<_, String>(Arc::new(Ready {
            session: session.clone(),
            client,
            server_stop,
        }))
    }
    .await;
    match result {
        Ok(ready) if !*owner.cancel.borrow() => Ok(ready),
        Ok(ready) => {
            let cleanup = ready.close().await.err().map(|_| session.clone());
            Err(StartupError {
                message: "LARM session cancelled".into(),
                cleanup,
            })
        }
        Err(message) => {
            let cleanup = session.close().await.err().map(|_| session.clone());
            Err(StartupError { message, cleanup })
        }
    }
}
struct StartupError {
    message: String,
    cleanup: Option<Arc<Session>>,
}
impl StartupError {
    async fn release(&self) -> Result<(), String> {
        match &self.cleanup {
            Some(session) => session.close().await.map_err(str::to_string),
            None => Ok(()),
        }
    }
}

impl Ready {
    async fn close(&self) -> Result<(), String> {
        self.server_stop.send_replace(true);
        self.session.close().await.map_err(str::to_string)
    }
}
#[tauri::command]
pub(crate) async fn end_larm_voice_session(
    state: tauri::State<'_, crate::AppState>,
    owner_id: String,
    drain: Option<bool>,
) -> Result<(), String> {
    if !enabled() {
        return Ok(());
    }
    if drain.unwrap_or(false) {
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(60);
        while state.streaming_tts.is_active() {
            if tokio::time::Instant::now() >= deadline {
                break; // The finally path still releases if speech cannot drain in time.
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
    }
    end(&owner_id).await
}
async fn end(owner_id: &str) -> Result<(), String> {
    let mut current = OWNER.lock().await;
    if current.as_ref().is_some_and(|o| o.id == owner_id) {
        let owner = current.as_ref().unwrap();
        owner.cancel.send_replace(true);
        // Wait for a pending initializer to release too; its create response may carry the id.
        let result = owner.ready.get_or_init(|| initialize(owner)).await;
        match result {
            Ok(ready) => ready.close().await?,
            Err(error) => error.release().await?,
        }
        *current = None;
    }
    Ok(())
}
pub(crate) async fn current(conversation: &str) -> Result<Arc<Ready>, String> {
    let owner = OWNER
        .lock()
        .await
        .clone()
        .ok_or("LARM voice session is not started")?;
    if owner.conversation != conversation || *owner.cancel.borrow() {
        return Err("LARM voice session mismatch".into());
    }
    owner
        .ready
        .get()
        .ok_or("LARM voice session is not ready")?
        .as_ref()
        .cloned()
        .map_err(|error| error.message.clone())
}
pub(crate) async fn shutdown() {
    SHUTTING_DOWN.store(true, Ordering::Release);
    let id = OWNER.lock().await.as_ref().map(|o| o.id.clone());
    if let Some(id) = id {
        if end(&id).await.is_err() {
            eprintln!("LARM session release failed on exit");
        }
    }
}
pub(crate) async fn reasoning_client(
    conversation: &str,
    origin: &str,
) -> Result<Option<Arc<crate::providers::reasoning_mcp::Client>>, String> {
    if !enabled() || origin != "voice" {
        return Ok(None);
    }
    Ok(Some(current(conversation).await?.client.clone()))
}
pub(crate) use decision::classify_shadow;

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[tokio::test]
    async fn disabled_mode_skips_session_lifecycle() {
        assert!(!enabled());
        begin_larm_voice_session("owner".into(), "conversation_primary".into())
            .await
            .expect("disabled begin is a no-op");
        assert!(current("conversation_primary").await.is_err());
        assert!(reasoning_client("conversation_primary", "text")
            .await
            .expect("text origin skips LARM")
            .is_none());
        assert!(reasoning_client("conversation_primary", "voice")
            .await
            .expect("disabled voice origin skips LARM")
            .is_none());
        classify_shadow(
            "conversation_primary",
            "hello",
            Arc::new(crate::RunCancellation::default()),
        )
        .await;
        shutdown().await;
    }
}
