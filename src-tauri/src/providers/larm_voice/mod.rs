//! Desktop-only owner: ephemeral credentials stay in Rust, shared with the embedded MCP service.
use rusqlite::{params, OptionalExtension};
use saaa_larm_session::Session;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, OnceLock,
};
use tokio::sync::{watch, Mutex, OnceCell};
pub(crate) mod audio;
mod decision;
pub(crate) mod frontdesk;
pub(crate) mod frontdesk_decision;
pub(crate) mod frontdesk_echo;
pub(crate) mod frontdesk_repository;
mod response;
pub(crate) mod speech_priority;
pub(crate) use response::{render as render_response, ResponseKind};

pub(crate) struct Ready {
    pub session: Arc<Session>,
}
struct Owner {
    id: String,
    conversation: String,
    base: String,
    profile: String,
    cancel: watch::Sender<bool>,
    ready: OnceCell<Result<Arc<Ready>, StartupError>>,
    started: AtomicBool,
    lease_key: String,
    sqlite_writer: Arc<crate::persistence::SqliteWriter>,
}
static OWNER: Mutex<Option<Arc<Owner>>> = Mutex::const_new(None);
static SHUTTING_DOWN: AtomicBool = AtomicBool::new(false);
static MODE: OnceLock<bool> = OnceLock::new();
pub(crate) fn enabled() -> bool {
    *MODE.get_or_init(|| {
        std::env::var("SAAA_CONVERSATION_REASONING_MODE")
            .map(|mode| mode != "off")
            .unwrap_or(true)
    })
}
#[tauri::command]
pub(crate) async fn begin_larm_voice_session(
    state: tauri::State<'_, crate::AppState>,
    owner_id: String,
    conversation_id: String,
) -> Result<(), String> {
    let harness = state
        .sqlite_readers
        .read(|c| Ok(crate::persistence::load_model_providers(c)?.harness))?;
    let base = harness.address;
    let profile = harness
        .larm_profile
        .unwrap_or_else(|| saaa_larm_session::DEFAULT_PROFILE.into());
    crate::validate_identifier(&owner_id, "voice owner")?;
    crate::validate_identifier(&conversation_id, "conversation id")?;
    let lease_key = current_lease_key(&state.sqlite_writer)?;
    let mut current = OWNER.lock().await;
    if SHUTTING_DOWN.load(Ordering::Acquire) {
        return Err("LARM voice runtime is shutting down".into());
    }
    if let Some(previous) = current.as_ref() {
        let inactive = match previous.ready.get() {
            Some(Ok(ready)) => !ready
                .session
                .ensure_active()
                .await
                .map_err(str::to_string)?,
            Some(Err(_)) => true,
            None => false,
        };
        if previous.id != owner_id
            || previous.conversation != conversation_id
            || previous.base != base
            || previous.profile != profile
            || inactive
        {
            previous.cancel.send_replace(true);
            close_owner(previous).await?;
            *current = None;
        }
    }
    current.get_or_insert_with(|| {
        let (cancel, _) = watch::channel(false);
        Arc::new(Owner {
            id: owner_id,
            conversation: conversation_id.clone(),
            base,
            profile,
            cancel,
            ready: OnceCell::new(),
            started: AtomicBool::new(false),
            lease_key,
            sqlite_writer: state.sqlite_writer.clone(),
        })
    });
    drop(current);
    // Microphone readiness includes the complete claimed provider set.
    self::current(&conversation_id)
        .await
        .map(|_| ())
        .map_err(|error| format!("larm-session-prepare-failed: {error}"))
}
async fn close_owner(owner: &Owner) -> Result<(), String> {
    if owner.started.load(Ordering::Acquire) {
        match owner.ready.get_or_init(|| initialize(owner)).await {
            Ok(ready) => ready.close().await?,
            Err(error) => error.release().await?,
        };
        rotate_lease_key(&owner.sqlite_writer, &owner.lease_key)?;
        Ok(())
    } else {
        Ok(())
    }
}

async fn initialize(owner: &Owner) -> Result<Arc<Ready>, StartupError> {
    #[cfg(not(test))]
    let credential =
        crate::providers::dynamic_lan::credential::load().map_err(|error| StartupError {
            message: error.code().into(),
            cleanup: None,
        })?;
    #[cfg(not(test))]
    let control_token = credential.token().to_string();
    #[cfg(test)]
    let control_token = "test-control-token".to_string();
    let session = Session::connect_with_profile_credential_and_key(
        &owner.base,
        &owner.profile,
        control_token,
        owner.lease_key.clone(),
        owner.cancel.subscribe(),
    )
    .await
    .map_err(|error| StartupError {
        message: error.to_string(),
        cleanup: error.cleanup,
    })?;
    if *owner.cancel.borrow() {
        let cleanup = session.close().await.err().map(|_| session.clone());
        return Err(StartupError {
            message: "LARM session cancelled".into(),
            cleanup,
        });
    }
    Ok(Arc::new(Ready { session }))
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
        self.session.close().await.map_err(str::to_string)
    }
}
#[tauri::command]
pub(crate) async fn end_larm_voice_session(
    state: tauri::State<'_, crate::AppState>,
    owner_id: String,
    drain: Option<bool>,
) -> Result<(), String> {
    if let Some(owner) = OWNER.lock().await.as_ref().filter(|o| o.id == owner_id) {
        speech_priority::stop_conversation(&state.streaming_tts, &owner.conversation);
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
        let Some(owner) = current.as_ref() else {
            return Ok(());
        };
        owner.cancel.send_replace(true);
        close_owner(owner).await?;
        *current = None;
    }
    Ok(())
}
pub(crate) async fn current_at(
    conversation: &str,
    settings: &crate::HarnessSettings,
) -> Result<Arc<Ready>, String> {
    let profile = settings
        .larm_profile
        .as_deref()
        .unwrap_or(saaa_larm_session::DEFAULT_PROFILE);
    {
        let mut slot = OWNER.lock().await;
        let owner = slot.as_ref().ok_or("LARM voice session is not started")?;
        if owner.conversation != conversation {
            return Err("LARM voice session mismatch".into());
        }
        let inactive = match owner.ready.get() {
            Some(Ok(ready)) => !ready
                .session
                .ensure_active()
                .await
                .map_err(str::to_string)?,
            Some(Err(_)) => true,
            None => false,
        };
        if owner.base != settings.address || owner.profile != profile || inactive {
            owner.cancel.send_replace(true);
            close_owner(owner).await?;
            let lease_key = current_lease_key(&owner.sqlite_writer)?;
            let (cancel, _) = watch::channel(false);
            *slot = Some(Arc::new(Owner {
                id: owner.id.clone(),
                conversation: conversation.into(),
                base: settings.address.clone(),
                profile: profile.into(),
                cancel,
                ready: OnceCell::new(),
                started: AtomicBool::new(false),
                lease_key,
                sqlite_writer: owner.sqlite_writer.clone(),
            }));
        }
    }
    current(conversation).await
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
    owner.started.store(true, Ordering::Release);
    // Startup retains cleanup ownership if the caller reaches its request deadline.
    tokio::spawn(async move {
        owner
            .ready
            .get_or_init(|| initialize(&owner))
            .await
            .as_ref()
            .cloned()
            .map_err(|e| e.message.clone())
    })
    .await
    .map_err(|_| "LARM startup worker stopped".to_string())?
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

fn current_lease_key(writer: &crate::persistence::SqliteWriter) -> Result<String, String> {
    writer.write(|connection| {
        let existing = connection
            .query_row(
                "SELECT idempotency_key FROM larm_voice_lease_slot WHERE id=1",
                [],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(crate::database_error)?;
        if let Some(key) = existing {
            return Ok(key);
        }
        let key = format!("saaa-voice-{}", uuid::Uuid::new_v4().simple());
        connection
            .execute(
                "INSERT INTO larm_voice_lease_slot(id,idempotency_key,updated_at) VALUES(1,?1,?2)",
                params![key, crate::now_iso()],
            )
            .map_err(crate::database_error)?;
        Ok(key)
    })
}

fn rotate_lease_key(
    writer: &crate::persistence::SqliteWriter,
    expected: &str,
) -> Result<(), String> {
    let next = format!("saaa-voice-{}", uuid::Uuid::new_v4().simple());
    writer.write(|connection| {
        connection
            .execute(
                "UPDATE larm_voice_lease_slot SET idempotency_key=?1,updated_at=?2 WHERE id=1 AND idempotency_key=?3",
                params![next, crate::now_iso(), expected],
            )
            .map_err(crate::database_error)?;
        Ok(())
    })
}
pub(crate) use decision::classify_shadow;

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;
    use std::sync::Arc;

    #[test]
    fn lease_slot_survives_restart_and_rotates_only_after_release() {
        let connection = Connection::open_in_memory().expect("database opens");
        crate::persistence::schema::initialize_database(&connection).expect("schema initializes");
        let writer = crate::persistence::SqliteWriter::from_connection(connection);
        let first = current_lease_key(&writer).expect("lease slot is created");
        assert_eq!(
            current_lease_key(&writer).expect("lease key should remain readable"),
            first
        );
        rotate_lease_key(&writer, &first).expect("confirmed release rotates the slot");
        let second = current_lease_key(&writer).expect("rotated lease key should be readable");
        assert_ne!(second, first);
        rotate_lease_key(&writer, &first).expect("stale release is harmless");
        assert_eq!(
            current_lease_key(&writer).expect("unchanged lease key should remain readable"),
            second
        );
    }

    #[tokio::test]
    async fn missing_owner_skips_session_lifecycle() {
        assert!(current("conversation_primary").await.is_err());
        classify_shadow(
            "conversation_primary",
            "hello",
            Arc::new(crate::RunCancellation::default()),
        )
        .await;
        shutdown().await;
    }
}

#[cfg(test)]
mod frontdesk_repository_tests;
mod world_tests;
