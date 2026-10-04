//! Ornith and speech adapters for the durable task queue.
use super::*;
use crate::task_queue::{self, Job, JobStatus};
use serde_json::json;
use tauri::{Emitter, Manager, Runtime};
#[path = "queue_answer_stream.rs"]
mod queue_answer_stream;
#[path = "queue_context.rs"]
mod queue_context;
#[path = "queue_input_state.rs"]
mod queue_input_state;
#[path = "queue_progress.rs"]
mod queue_progress;
#[path = "queue_recovery.rs"]
mod queue_recovery;

type JobCancellations = std::collections::HashMap<String, (String, Arc<RunCancellation>)>;
static JOB_CANCEL: std::sync::OnceLock<std::sync::Mutex<JobCancellations>> =
    std::sync::OnceLock::new();

struct JobCancelGuard(String);

impl Drop for JobCancelGuard {
    fn drop(&mut self) {
        if let Some(registry) = JOB_CANCEL.get() {
            if let Ok(mut registry) = registry.lock() {
                registry.remove(&self.0);
            }
        }
    }
}

fn register_job_cancel(
    job: &Job,
    cancellation: Arc<RunCancellation>,
) -> Result<JobCancelGuard, String> {
    JOB_CANCEL
        .get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()))
        .lock()
        .map_err(|_| "会話処理の取消し状態を取得できません。")?
        .insert(job.id.clone(), (job.key.clone(), cancellation));
    Ok(JobCancelGuard(job.id.clone()))
}

fn cancel_generation(input_id: &str) {
    if let Some(registry) = JOB_CANCEL.get() {
        if let Ok(registry) = registry.lock() {
            for (key, cancellation) in registry.values() {
                if key == input_id {
                    cancellation.cancel();
                }
            }
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct QueueReceipt {
    pub(crate) input_id: String,
    pub(crate) job_id: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct QueueSnapshot {
    pub(crate) jobs: Vec<JobStatus>,
    pub(crate) speech_playing: bool,
}

#[tauri::command]
pub(crate) fn start_conversation_audio_idle() -> Result<(), String> {
    crate::voice::local_audio_output::start_idle_output()
}

#[tauri::command]
pub(crate) fn stop_conversation_audio_idle() {
    crate::voice::local_audio_output::stop_idle_output();
}

#[tauri::command]
pub(crate) fn conversation_audio_idle_status() -> crate::voice::local_audio_output::IdleOutputStatus
{
    crate::voice::local_audio_output::idle_output_status()
}

#[tauri::command]
pub(crate) fn enqueue_conversation_text(
    state: tauri::State<'_, AppState>,
    app: tauri::AppHandle,
    input_id: String,
    text: String,
) -> Result<QueueReceipt, String> {
    let receipt = enqueue_text(&state, input_id, text)?;
    state.conversation_queue_wake.notify_waiters();
    let _ = app.emit("conversation-queue-updated", ());
    Ok(receipt)
}

pub(crate) fn enqueue_text(
    state: &AppState,
    input_id: String,
    text: String,
) -> Result<QueueReceipt, String> {
    validate_identifier(&input_id, "input id")?;
    if text.trim().is_empty() || text.len() > 4096 {
        return Err("入力は1〜4096バイトにしてください。".into());
    }
    crate::memory::personal_state::worker::interrupt();
    let payload = json!({"text": text}).to_string();
    let user_id = format!("check_{input_id}");
    let job_id = state.sqlite_writer.write(|connection| {
        let transaction = connection.transaction().map_err(database_error)?;
        let previous: Option<String> = transaction.query_row(
            "SELECT content FROM conversation_messages WHERE id=?1 AND conversation_id=?2 AND role='user'",
            params![user_id, PRIMARY_CONVERSATION_ID], |row| row.get(0),
        ).optional().map_err(database_error)?;
        if let Some(previous) = previous {
            if previous != text { return Err("同じ入力IDで異なる本文は送信できません。".into()); }
        } else {
            let pending: i64 = transaction.query_row("SELECT count(DISTINCT job_key) FROM task_queue_jobs WHERE scope=?1 AND state IN ('queued','running')", [PRIMARY_CONVERSATION_ID], |row| row.get(0)).map_err(database_error)?;
            if pending >= 16 { return Err("会話の処理キューが満杯です。少し待ってから再送してください。".into()); }
            transaction.execute(
                "INSERT INTO conversation_messages(id,conversation_id,role,content,created_at) VALUES(?1,?2,'user',?3,?4)",
                params![user_id, PRIMARY_CONVERSATION_ID, text, now_iso()],
            ).map_err(database_error)?;
        }
        let id = task_queue::enqueue(&transaction, PRIMARY_CONVERSATION_ID, "conversation", "user_input", &input_id, 0, &payload,Some(16))?;
        let run_id = format!("run_{input_id}");
        let exists: bool = transaction.query_row(
            "SELECT EXISTS(SELECT 1 FROM runtime_runs WHERE id=?1)", [&run_id], |row| row.get(0),
        ).map_err(database_error)?;
        if !exists {
            transaction.execute(
                "INSERT INTO runtime_runs(id,conversation_id,route_kind,status,input_message_id,started_at) VALUES(?1,?2,'conversation.respond','running',?3,?4)",
                params![run_id, PRIMARY_CONVERSATION_ID, user_id, now_iso()],
            ).map_err(database_error)?;
            let context_input = StartTurnInput {
                run_id, conversation_id: PRIMARY_CONVERSATION_ID.into(), content: text.clone(),
                workspace_path: None, retry_input_message_id: None, source_id: None,
                scope_refs: Vec::new(), input_origin: "text".into(), presentation_mode: "visual-and-spoken".into(),
            };
            let scope = crate::runtime::context::scope::resolve(&transaction, &context_input, &user_id, true)?;
            if scope.status != "resolved" { return Err("会話のスコープを確定できません。".into()); }
        }
        transaction.commit().map_err(database_error)?;
        Ok(id)
    })?;
    Ok(QueueReceipt { input_id, job_id })
}

#[tauri::command]
pub(crate) fn cancel_conversation_input(
    state: tauri::State<'_, AppState>,
    app: tauri::AppHandle,
    input_id: String,
) -> Result<(), String> {
    cancel_input(&state, &input_id)?;
    let _ = app.emit("conversation-queue-updated", ());
    Ok(())
}

pub(crate) fn cancel_input(state: &AppState, input_id: &str) -> Result<(), String> {
    validate_identifier(input_id, "input id")?;
    state.sqlite_writer.write(|connection| {
        let tx = connection.transaction().map_err(database_error)?;
        queue_input_state::cancel(&tx, PRIMARY_CONVERSATION_ID, input_id, &now_iso())?;
        tx.commit().map_err(database_error)
    })?;
    state
        .tts_dictionary_cache
        .proposals
        .cancel_run(&format!("run_{input_id}"));
    cancel_generation(input_id);
    super::cancel_active_speech(input_id);
    state.conversation_queue_wake.notify_waiters();
    Ok(())
}

#[tauri::command]
pub(crate) fn conversation_queue_snapshot(
    state: tauri::State<'_, AppState>,
) -> Result<QueueSnapshot, String> {
    state.sqlite_readers.read(|connection| {
        Ok(QueueSnapshot {
            jobs: task_queue::snapshot(connection, PRIMARY_CONVERSATION_ID)?,
            speech_playing: super::speech_playing(),
        })
    })
}

#[tauri::command]
pub(crate) async fn replay_conversation_speech(
    state: tauri::State<'_, AppState>,
    app: tauri::AppHandle,
    input_id: String,
) -> Result<(), String> {
    validate_identifier(&input_id, "input id")?;
    let speech = state.sqlite_writer.write(|connection| {
        let tx = connection.transaction().map_err(database_error)?;
        let prior: Option<(String, i64, String)> = tx.query_row(
            "SELECT id,generation,payload_json FROM task_queue_jobs WHERE scope=?1 AND kind='speech' AND job_key=?2 AND state='interrupted'",
            params![PRIMARY_CONVERSATION_ID,input_id],
            |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?)),
        ).optional().map_err(database_error)?;
        let (id,generation,payload) = prior.ok_or("再生できる音声がありません。")?;
        let owner = uuid::Uuid::new_v4().simple().to_string();
        let changed = tx.execute(
            "UPDATE task_queue_jobs SET state='running',owner=?2,attempts=attempts+1,lease_until_ms=?3,updated_at_ms=?4 WHERE id=?1 AND state='interrupted'",
            params![id,owner,task_queue::now_ms()+180_000,task_queue::now_ms()],
        ).map_err(database_error)?;
        if changed != 1 { return Err("再生できる音声がありません。".into()); }
        tx.commit().map_err(database_error)?;
        Ok(Job { id,scope:PRIMARY_CONVERSATION_ID.into(),lane:"speech".into(),kind:"speech".into(),key:input_id.clone(),generation,payload,owner })
    })?;
    let _ = app.emit("conversation-queue-updated", ());
    let audit = ConversationAudit::new(state.sqlite_writer.clone(), input_id.clone());
    let result =
        speak_conversation_answer_inner(&state, &app, &input_id, &audit, Some(&speech)).await;
    finish_stream_speech_job(
        &state,
        &speech,
        result.as_ref().map(|_| ()).map_err(String::as_str),
    )?;
    let _ = app.emit("conversation-queue-updated", ());
    result
}

pub(crate) fn spawn<R: Runtime>(app: tauri::AppHandle<R>) {
    if let Err(error) = app.state::<AppState>().sqlite_writer.write(|connection| {
        let tx = connection.transaction().map_err(database_error)?;
        migrate_legacy_jobs(&tx)?;
        tx.execute(
            "UPDATE runtime_runs SET status='running',completed_at=NULL
             WHERE status='interrupted' AND id IN (
               SELECT 'run_' || job_key FROM task_queue_jobs
               WHERE lane='conversation' AND state IN ('queued','running'))",
            [],
        )
        .map_err(database_error)?;
        queue_recovery::mark_uncertain_speech(
            &tx,
            PRIMARY_CONVERSATION_ID,
            &now_iso(),
            task_queue::now_ms(),
        )?;
        queue_progress::recover(&tx, PRIMARY_CONVERSATION_ID)?;
        tx.commit().map_err(database_error)
    }) {
        eprintln!("conversation queue context recovery: {error}");
    }
    // Multiple conversation workers let a cancellation or replacement overtake a slow model call.
    for lane in [
        "conversation",
        "conversation",
        "conversation",
        "conversation",
        "speech",
    ] {
        let app = app.clone();
        tauri::async_runtime::spawn(async move { run_lane(app, lane).await });
    }
}

pub(crate) fn migrate_legacy_jobs(connection: &rusqlite::Connection) -> Result<(), String> {
    // The completed old user_input remains for input ordering and idempotency. Its Ornith task
    // needs a distinct kind because the queue's uniqueness key omits the lane.
    connection.execute(
        "UPDATE task_queue_jobs SET lane='conversation',kind='conversation_resume',owner=NULL,lease_until_ms=NULL,state='queued'
         WHERE lane='ornith' AND kind='ornith_task' AND state IN ('queued','running')",
        [],
    ).map_err(database_error)?;
    connection.execute(
        "UPDATE task_queue_jobs SET lane='conversation',owner=NULL,lease_until_ms=NULL,state='queued'
         WHERE lane='qwen' AND kind IN ('user_input','ornith_result') AND state IN ('queued','running')",
        [],
    ).map_err(database_error)?;
    Ok(())
}

async fn run_lane<R: Runtime>(app: tauri::AppHandle<R>, lane: &'static str) {
    loop {
        let wake = app.state::<AppState>().conversation_queue_wake.clone();
        let notified = wake.notified();
        tokio::pin!(notified);
        notified.as_mut().enable();
        loop {
            let claimed = app
                .state::<AppState>()
                .sqlite_writer
                .write(|connection| task_queue::claim(connection, lane));
            let job = match claimed {
                Ok(Some(job)) => job,
                Ok(None) => break,
                Err(error) => {
                    eprintln!("conversation queue claim: {error}");
                    break;
                }
            };
            let _ = app.emit("conversation-queue-updated", ());
            let result = process_job(&app, &job).await;
            if let Err(error) = result {
                if lane == "speech" {
                    if let Err(persist_error) =
                        finish_stream_speech_job(&app.state::<AppState>(), &job, Err(&error))
                    {
                        eprintln!("conversation speech interruption persistence: {persist_error}");
                    }
                    app.state::<AppState>()
                        .conversation_queue_wake
                        .notify_waiters();
                    let _ = app.emit("conversation-queue-updated", ());
                    continue;
                }
                let persisted = app.state::<AppState>().sqlite_writer.write(|connection| {
                    let tx = connection.transaction().map_err(database_error)?;
                    let cancelled: bool = tx.query_row(
                        "SELECT state='cancelled' FROM task_queue_jobs WHERE id=?1",
                        [&job.id], |row| row.get(0),
                    ).map_err(database_error)?;
                    if cancelled { tx.commit().map_err(database_error)?; return Ok(false); }
                    task_queue::fail(&tx, &job, &error)?;
                    let state: String = tx.query_row("SELECT state FROM task_queue_jobs WHERE id=?1",[&job.id],|row| row.get(0)).map_err(database_error)?;
                    let terminal_result = state == "failed";
                    if terminal_result { queue_progress::cancel(&tx, &job.scope, &job.key)?; }
                    if state == "failed" {
                        tx.execute("UPDATE runtime_runs SET status='failed',error_message=?2,completed_at=?3 WHERE id=?1 AND status='running'",
                            params![format!("run_{}",job.key),error.chars().take(500).collect::<String>(),now_iso()]).map_err(database_error)?;
                    }
                    tx.commit().map_err(database_error)?;
                    Ok(terminal_result)
                });
                match persisted {
                    Ok(true) => super::cancel_active_progress_speech(&job.key),
                    Ok(false) => {}
                    Err(persist_error) => {
                        eprintln!("conversation queue failure persistence: {persist_error}")
                    }
                }
            }
            app.state::<AppState>()
                .conversation_queue_wake
                .notify_waiters();
            let _ = app.emit("conversation-queue-updated", ());
        }
        tokio::select! {
            _ = &mut notified => {},
            _ = tokio::time::sleep(std::time::Duration::from_secs(1)) => {},
        }
    }
}

fn active_previous(
    state: &AppState,
    current_key: &str,
) -> Result<Option<(String, String)>, String> {
    state.sqlite_readers.read(|connection| {
        queue_input_state::active_previous(connection, PRIMARY_CONVERSATION_ID, current_key)
    })
}

async fn process_job<R: Runtime>(app: &tauri::AppHandle<R>, job: &Job) -> Result<(), String> {
    if job.kind == "terminal_question" {
        return super::terminal_decision::process(app, job).await;
    }
    if job.lane == "speech" {
        return if job.kind == "progress_speech" {
            process_progress_speech(app, job).await
        } else {
            process_speech(app, job).await
        };
    }
    let cancellation = Arc::new(RunCancellation::default());
    let _guard = register_job_cancel(job, cancellation.clone())?;
    // Register before checking the durable state: cancellation may race with claiming a job.
    let current = app.state::<AppState>().sqlite_readers.read(|connection| {
        connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM task_queue_jobs WHERE id=?1 AND state='running' AND owner=?2 AND generation=?3)",
            params![job.id, job.owner, job.generation], |row| row.get::<_, bool>(0),
        ).map_err(database_error)
    })?;
    if !current {
        return Ok(());
    }
    tokio::select! {
        biased;
        _ = cancellation.cancelled() => Err("会話の処理は中止されました。".into()),
        result = async {
            match job.lane.as_str() {
                "conversation" => process_conversation(app, job, cancellation.clone()).await,
                _ => Err("未対応の処理キューです。".into()),
            }
        } => result,
    }
}

async fn process_conversation<R: Runtime>(
    app: &tauri::AppHandle<R>,
    job: &Job,
    cancellation: Arc<RunCancellation>,
) -> Result<(), String> {
    let state = app.state::<AppState>();
    let payload: Value =
        serde_json::from_str(&job.payload).map_err(|_| "キューデータが不正です。")?;
    if job.kind == "user_input" || job.kind == "conversation_resume" {
        let text = payload["text"].as_str().ok_or("入力本文がありません。")?;
        let previous = active_previous(&state, &job.key)?;
        let normalized = text.trim().trim_end_matches(['。', '！', '!', ' ']);
        let cancel_request = previous.is_some()
            && ["やめて", "中止して", "キャンセルして", "その依頼を中止して"].contains(&normalized);
        if cancel_request {
            let cancel = previous.as_ref().map(|v| v.0.as_str());
            commit_answer(
                &state,
                job,
                "承知しました。前の依頼を中止しました。".into(),
                cancel,
                &[],
                None,
                None,
                None,
            )?;
            if let Some(key) = cancel {
                cancel_generation(key);
                super::cancel_active_speech(key);
            }
        } else {
            let replace = previous.is_some()
                && ["代わりに", "前の依頼を訂正", "今の依頼を訂正", "訂正して、"]
                    .iter()
                    .any(|marker| text.trim().starts_with(marker));
            if replace {
                state.sqlite_writer.write(|connection| {
                    let tx = connection.transaction().map_err(database_error)?;
                    if let Some((key, _)) = previous.as_ref() {
                        queue_input_state::cancel(&tx, &job.scope, key, &now_iso())?;
                    }
                    tx.commit().map_err(database_error)
                })?;
                if let Some((key, _)) = previous {
                    cancel_generation(&key);
                    super::cancel_active_speech(&key);
                }
            }
            let audio_started = Arc::new(AtomicBool::new(false));
            let retry_blocked = Arc::new(AtomicBool::new(false));
            let answer = match process_ornith(
                app,
                job,
                cancellation,
                audio_started.clone(),
                retry_blocked.clone(),
            )
            .await
            {
                Ok(answer) => answer,
                Err(error)
                    if audio_started.load(Ordering::Acquire)
                        || retry_blocked.load(Ordering::Acquire) =>
                {
                    crate::tts_dictionary::tools::finish_turn(
                        &state,
                        &job.scope,
                        &format!("run_{}", job.key),
                        false,
                    );
                    fail_answer_terminal(&state, job, &error)?;
                    return Ok(());
                }
                Err(error) => {
                    crate::tts_dictionary::tools::finish_turn(
                        &state,
                        &job.scope,
                        &format!("run_{}", job.key),
                        false,
                    );
                    return Err(error);
                }
            };
            let committed = commit_answer(
                &state,
                job,
                answer.content,
                None,
                &answer.source_urls,
                Some(&answer.context),
                answer.speech.as_ref(),
                answer.route.as_ref().map(|route| (route, answer.deadline)),
            );
            crate::tts_dictionary::tools::finish_turn(
                &state,
                &job.scope,
                &format!("run_{}", job.key),
                committed.is_ok(),
            );
            if let Err(error) = committed {
                // Saving is the final acceptance boundary. Never regenerate a
                // displayed/spoken answer or repeat its tools after this fails.
                fail_answer_terminal(&state, job, &error)?;
                return Ok(());
            }
            if app.emit("conversation-queue-updated", ()).is_ok() {
                if let Some(metrics) = answer.publication {
                    metrics.visible();
                }
            }
            super::cancel_active_progress_speech(&job.key);
        }
    } else if job.kind == "ornith_result" {
        let result = match payload["status"].as_str() {
            Some("failed") => format!(
                "調査は失敗しました: {}",
                payload["error"]
                    .as_str()
                    .filter(|error| !error.trim().is_empty())
                    .unwrap_or("原因を特定できませんでした。")
            ),
            Some("completed") | None => payload["result"]
                .as_str()
                .ok_or("Ornithの結果がありません。")?
                .to_string(),
            _ => return Err("Ornithの結果状態が不正です。".into()),
        };
        let source_urls = payload["sourceUrls"]
            .as_array()
            .map(|urls| {
                urls.iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        // This lane delivers an already verified result; it must not generate it again.
        let committed = (|| {
            let context = payload["contextDigest"]
                .as_str()
                .map(|expected| {
                    let context = queue_context::compose(&state, &job.key)?;
                    if context.fingerprint()? != expected {
                        return Err("回答の根拠が変更されたため公開できません。".to_string());
                    }
                    Ok(context)
                })
                .transpose()?;
            commit_answer(
                &state,
                job,
                result,
                None,
                &source_urls,
                context.as_ref(),
                None,
                None,
            )
        })();
        if committed.is_ok() {
            super::cancel_active_progress_speech(&job.key);
        }
        if let Err(error) = committed {
            let _ = app.emit("conversation-queue-updated", ());
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
            state.sqlite_writer.write(|connection| {
                let tx = connection.transaction().map_err(database_error)?;
                task_queue::fail_terminal(&tx, job, &error)?;
                queue_progress::cancel(&tx, &job.scope, &job.key)?;
                tx.execute(
                    "UPDATE runtime_runs SET status='failed',error_message=?2,completed_at=?3
                     WHERE id=?1 AND status='running'",
                    params![format!("run_{}", job.key), error, now_iso()],
                )
                .map_err(database_error)?;
                tx.commit().map_err(database_error)
            })?;
            super::cancel_active_progress_speech(&job.key);
            return Ok(());
        }
    } else {
        return Err("会話キューに未対応の仕事があります。".into());
    }
    Ok(())
}

fn fail_answer_terminal(state: &AppState, job: &Job, error: &str) -> Result<(), String> {
    state.sqlite_writer.write(|connection| {
        let tx = connection.transaction().map_err(database_error)?;
        let cancelled:bool=tx.query_row("SELECT state='cancelled' FROM task_queue_jobs WHERE id=?1",[&job.id],|r|r.get(0)).map_err(database_error)?;
        if !cancelled {
            task_queue::fail_terminal(&tx, job, error)?;
            queue_progress::cancel(&tx, &job.scope, &job.key)?;
            tx.execute("UPDATE runtime_runs SET status='failed',error_message=?2,completed_at=?3 WHERE id=?1 AND status='running'",params![format!("run_{}",job.key),error.chars().take(500).collect::<String>(),now_iso()]).map_err(database_error)?;
        }
        tx.commit().map_err(database_error)
    })
}

fn validate_answer_content(answer: &str) -> Result<(), String> {
    if answer.trim().is_empty()
        || answer.len() > MAX_ANSWER_BYTES
        || answer.contains("<think>")
        || answer.contains("</think>")
        || answer.contains("<|")
    {
        return Err("回答本文が空か、不正です。".into());
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn commit_answer(
    state: &AppState,
    job: &Job,
    answer: String,
    cancel_key: Option<&str>,
    source_urls: &[String],
    context: Option<&queue_context::QueueContext>,
    streamed_speech: Option<&super::streaming_speech::AnswerStreamReport>,
    acceptance: Option<(
        &crate::providers::service_registry::ResolvedRoute,
        tokio::time::Instant,
    )>,
) -> Result<(), String> {
    validate_answer_content(&answer)?;
    let answer = append_source_links(answer, source_urls);
    if answer.len() > MAX_ANSWER_BYTES {
        return Err("回答本文が長すぎます。".into());
    }
    let answer_id = format!("reply_{}", job.key);
    let context_digest = context.map(|context| context.fingerprint()).transpose()?;
    state.sqlite_writer.write(|connection| {
        let tx = connection.transaction().map_err(database_error)?;
        if let Some(context) = context { context.validate_commit(&tx)?; }
        if let Some((route, deadline))=acceptance {
            if tokio::time::Instant::now() >= deadline { return Err("この依頼の全体期限を超えたため回答を保存できません。".into()); }
            direct_route::validate_route(&tx,route)?;
        }
        let input: Value = serde_json::from_str(&job.payload).map_err(|_| "入力参照が不正です。")?;
        let current: String = tx.query_row("SELECT content FROM conversation_messages WHERE id=?1 AND conversation_id=?2 AND role='user'",
            params![format!("check_{}", job.key), job.scope], |row| row.get(0)).map_err(database_error)?;
        if input["text"].as_str() != Some(current.as_str()) { return Err("入力が変更されたため回答を公開できません。".into()); }
        queue_progress::cancel(&tx, &job.scope, &job.key)?;
        if let Some(key) = cancel_key {
            queue_input_state::cancel(&tx, &job.scope, key, &now_iso())?;
        }
        tx.execute(
            "INSERT OR IGNORE INTO conversation_messages(id,conversation_id,role,content,created_at) VALUES(?1,?2,'assistant',?3,?4)",
            params![answer_id, PRIMARY_CONVERSATION_ID, answer, now_iso()],
        ).map_err(database_error)?;
        let saved: String = tx.query_row("SELECT content FROM conversation_messages WHERE id=?1",[&answer_id],|row| row.get(0)).map_err(database_error)?;
        if saved != answer { return Err("同じ発話IDの回答本文が一致しません。".into()); }
        let speech_id = task_queue::enqueue(&tx,&job.scope,"speech","speech",&job.key,job.generation,
            &json!({"messageId":answer_id,"contextDigest":context_digest}).to_string(),None)?;
        if let Some(report) = streamed_speech.filter(|report| report.started || report.error.is_some()) {
            tx.execute(
                "UPDATE task_queue_jobs SET state=?2,error=?3,updated_at_ms=?4 WHERE id=?1 AND state='queued'",
                params![speech_id,
                    if report.error.is_some() { "interrupted" } else { "completed" },
                    report.error.as_deref(), task_queue::now_ms()],
            ).map_err(database_error)?;
        }
        task_queue::finish(&tx,job)?;
        let completed = tx.execute("UPDATE runtime_runs SET status='completed',completed_at=?2 WHERE id=?1 AND status='running'",
            params![format!("run_{}",job.key),now_iso()]).map_err(database_error)?;
        if completed != 1 { return Err("実行状態が変化したため回答を公開できません。".into()); }
        if let Some((route, _))=acceptance { crate::providers::service_registry::operations::accepted(&tx,&job.key,route)?; }
        tx.commit().map_err(database_error)?;
        Ok(())
    })
}

fn append_source_links(mut answer: String, urls: &[String]) -> String {
    let mut links = Vec::new();
    for raw in urls {
        let Ok(url) = url::Url::parse(raw) else {
            continue;
        };
        if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
            continue;
        }
        let href = url.as_str().replace(')', "%29");
        if links
            .iter()
            .any(|(_, existing): &(String, String)| existing == &href)
        {
            continue;
        }
        let label = format!(
            "出典{}: {}",
            links.len() + 1,
            url.host_str().unwrap_or_default()
        );
        links.push((label, href));
        if links.len() == 3 {
            break;
        }
    }
    if !links.is_empty() {
        answer.push_str(SOURCE_LINKS_MARKER);
        for (label, href) in links {
            answer.push_str(&format!("[{label}]({href})\n"));
        }
    }
    answer
}

fn finish_stream_speech_job(
    state: &AppState,
    speech: &Job,
    result: Result<(), &str>,
) -> Result<(), String> {
    state.sqlite_writer.write(|connection| {
        let outcome = match result {
            Ok(()) => task_queue::finish(connection, speech),
            Err(error) => {
                let changed = connection.execute(
                    "UPDATE task_queue_jobs SET state='interrupted',owner=NULL,lease_until_ms=NULL,error=?4,updated_at_ms=?5
                     WHERE id=?1 AND state='running' AND owner=?2 AND generation=?3",
                    params![speech.id,speech.owner,speech.generation,error.chars().take(500).collect::<String>(),task_queue::now_ms()],
                ).map_err(database_error)?;
                if changed == 1 { Ok(()) } else { Err("音声の仕事を確定できませんでした。".into()) }
            }
        };
        if outcome.is_err() {
            let cancelled: bool = connection.query_row(
                "SELECT state='cancelled' FROM task_queue_jobs WHERE id=?1",
                [&speech.id], |row| row.get(0),
            ).map_err(database_error)?;
            if cancelled { return Ok(()); }
        }
        outcome
    })
}

mod action;
#[path = "queue_runtime/ornith.rs"]
mod ornith;
use ornith::process_ornith;

#[path = "queue_runtime/speech.rs"]
mod speech;
pub(super) use speech::{
    cancel_progress_for_stream, progress_eligible, record_progress_message, stream_context_digest,
    validate_speech_context,
};
use speech::{process_progress_speech, process_speech};

/// Execute the production conversation lane once against an isolated evaluation DB.
#[cfg(feature = "quality-eval-harness")]
pub(crate) async fn evaluate_one<R: Runtime>(app: &tauri::AppHandle<R>) -> Result<(), String> {
    let job = app
        .state::<AppState>()
        .sqlite_writer
        .write(|c| task_queue::claim(c, "conversation"))?
        .ok_or("evaluation input was not claimed")?;
    process_job(app, &job).await
}
