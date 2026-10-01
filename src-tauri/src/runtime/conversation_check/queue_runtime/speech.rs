//! Speech jobs retain foreground admission and durable completion.
use super::*;
pub(crate) fn validate_speech_context(state: &AppState, job: &Job) -> Result<(), String> {
    let payload: Value =
        serde_json::from_str(&job.payload).map_err(|_| "音声の根拠参照が不正です。")?;
    if let Some(expected) = payload["contextDigest"].as_str() {
        let context = queue_context::compose(state, &job.key)?;
        if context.fingerprint()? != expected {
            return Err("回答の根拠が変更されたため読み上げできません。".into());
        }
        context.validate_result(state)?;
    }
    Ok(())
}

pub(super) async fn process_speech<R: Runtime>(
    app: &tauri::AppHandle<R>,
    job: &Job,
) -> Result<(), String> {
    let _personal_slot = crate::memory::personal_state::worker::foreground().await;
    let state = app.state::<AppState>();
    let audit = ConversationAudit::new(state.sqlite_writer.clone(), job.key.clone());
    speak_conversation_answer_inner(&state, app, &job.key, &audit, Some(job)).await?;
    state
        .sqlite_writer
        .write(|connection| task_queue::finish(connection, job))
}

pub(super) async fn process_progress_speech<R: Runtime>(
    app: &tauri::AppHandle<R>,
    job: &Job,
) -> Result<(), String> {
    let _personal_slot = crate::memory::personal_state::worker::foreground().await;
    let state = app.state::<AppState>();
    let eligible = state
        .sqlite_readers
        .read(|connection| queue_progress::eligible(connection, &job.scope, &job.key))?;
    if !eligible {
        state
            .sqlite_writer
            .write(|connection| task_queue::finish(connection, job))?;
        return Ok(());
    }
    let audit = ConversationAudit::new(state.sqlite_writer.clone(), job.key.clone());
    let played = super::super::streaming_speech::play_progress(&state, app, job, &audit).await;
    let cancelled = state.sqlite_readers.read(|connection| {
        connection
            .query_row(
                "SELECT state='cancelled' FROM task_queue_jobs WHERE id=?1",
                [&job.id],
                |row| row.get::<_, bool>(0),
            )
            .map_err(database_error)
    })?;
    if cancelled {
        return Ok(());
    }
    played?;
    state.sqlite_writer.write(|connection| {
        let tx = connection.transaction().map_err(database_error)?;
        queue_progress::finish(&tx, job)?;
        tx.commit().map_err(database_error)
    })
}

pub(crate) fn progress_eligible(state: &AppState, job: &Job) -> Result<bool, String> {
    state
        .sqlite_readers
        .read(|connection| queue_progress::eligible(connection, &job.scope, &job.key))
}

pub(crate) fn cancel_progress_for_stream(state: &AppState, key: &str) -> Result<(), String> {
    state
        .sqlite_writer
        .write(|connection| queue_progress::cancel(connection, PRIMARY_CONVERSATION_ID, key))
}

pub(crate) fn stream_context_digest(state: &AppState, key: &str) -> Result<String, String> {
    queue_context::compose(state, key)?.fingerprint()
}

pub(crate) fn record_progress_message(
    state: &AppState,
    job: &Job,
    text: &str,
) -> Result<bool, String> {
    state.sqlite_writer.write(|connection| {
        let tx = connection.transaction().map_err(database_error)?;
        let recorded = queue_progress::record_message(&tx, job, text, &now_iso())?;
        tx.commit().map_err(database_error)?;
        Ok(recorded)
    })
}
