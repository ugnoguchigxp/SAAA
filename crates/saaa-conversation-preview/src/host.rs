use crate::{
    config::{self, PreviewConfig, TtsSelection},
    credential, isolation, qwen, repository, speech,
};
use rusqlite::{Connection, OptionalExtension};
use saaa_conversation_core::{
    contracts::{Action, TextInput},
    frontdesk::{Frontdesk, State as FrontdeskState},
    qwen::Event,
    speaking::{Clause, Speaking, State as SpeechState},
};
use serde::Serialize;
use std::{
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};
use tauri::{AppHandle, Emitter};
use tokio::{
    sync::{mpsc, watch, Mutex as AsyncMutex},
    task::JoinHandle,
    time::Instant,
};

pub struct PreviewApp {
    active: AsyncMutex<Option<Arc<Host>>>,
}

impl Default for PreviewApp {
    fn default() -> Self {
        Self {
            active: AsyncMutex::new(None),
        }
    }
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    session_id: String,
    version: u64,
    resource_state: String,
    resource_phase: String,
    resource_elapsed_ms: u64,
    model: String,
    tts: TtsSelection,
    response: Option<ResponseSnapshot>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ResponseSnapshot {
    response_id: String,
    input_id: String,
    input_text: String,
    action: Option<String>,
    frontdesk_state: String,
    speech_state: String,
    public_text: String,
    failure_stage: Option<String>,
    failure_code: Option<String>,
    last_played_clause: Option<u32>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SubmitReceipt {
    response_id: String,
    input_id: String,
    duplicate: bool,
}

pub struct Host {
    app: AppHandle,
    db: Mutex<Connection>,
    data_directory: PathBuf,
    session_id: String,
    config: PreviewConfig,
    phase: Mutex<String>,
    started_at: Instant,
    model: Mutex<String>,
    resource: Mutex<Option<Arc<saaa_larm_session::Session>>>,
    resource_cancel: watch::Sender<bool>,
    prepare_task: Mutex<Option<JoinHandle<()>>>,
    response: Mutex<Option<Arc<ResponseRun>>>,
    admission: Mutex<()>,
    shutdown: AsyncMutex<()>,
    version: AtomicU64,
    blocked: AtomicBool,
}

struct ResponseRun {
    response_id: String,
    frontdesk: Mutex<Frontdesk>,
    speaking: Mutex<Speaking>,
    speech_tx: mpsc::Sender<SpeechCommand>,
    qwen_cancel: watch::Sender<bool>,
    speech_cancel: watch::Sender<bool>,
    qwen_task: Mutex<Option<JoinHandle<()>>>,
    speech_task: Mutex<Option<JoinHandle<()>>>,
    delta_seq: AtomicU64,
    persisted_bytes: AtomicUsize,
    speech_deadline: Instant,
}

enum SpeechCommand {
    Clause(Clause),
    Finish,
}

impl PreviewApp {
    pub async fn open(&self, app: AppHandle) -> Result<Snapshot, String> {
        let mut active = self.active.lock().await;
        if let Some(host) = active.as_ref() {
            return host.snapshot();
        }
        let (mut db, data_directory) =
            tokio::task::spawn_blocking(isolation::open_isolated_database)
                .await
                .map_err(|_| "隔離DBを準備できません。")??;
        repository::migrate(&db).map_err(|_| "preview schemaを作成できません。")?;
        let schema_version: i64 = db
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .map_err(|_| "preview schema versionを読み取れません。")?;
        db.pragma_update(None, "user_version", schema_version.max(42))
            .map_err(|_| "preview schema versionを保存できません。")?;
        repository::interrupt_after_restart(&mut db, &now())
            .map_err(|_| "前回のpreview応答を中断できません。")?;
        let config = config::load(&db)?;
        if isolation::source_config_fingerprint()? != config.fingerprint {
            return Err(
                "コピー中に保存済み設定が変わりました。新しい隔離ディレクトリで開始してください。"
                    .into(),
            );
        }
        let session_id = format!("preview-{}", uuid::Uuid::new_v4().simple());
        let conversation_id = format!("conversation-{}", uuid::Uuid::new_v4().simple());
        let larm_key = format!("saaa-preview-{}", uuid::Uuid::new_v4().simple());
        repository::open_session(
            &mut db,
            &session_id,
            &conversation_id,
            &config.fingerprint,
            &larm_key,
            &now(),
        )
        .map_err(|_| "preview sessionを保存できません。")?;
        let (resource_cancel, receiver) = watch::channel(false);
        let host = Arc::new(Host {
            app,
            db: Mutex::new(db),
            data_directory,
            session_id,
            model: Mutex::new(config.profile_label.clone()),
            config,
            phase: Mutex::new("model_preparing".into()),
            started_at: Instant::now(),
            resource: Mutex::new(None),
            resource_cancel,
            prepare_task: Mutex::new(None),
            response: Mutex::new(None),
            admission: Mutex::new(()),
            shutdown: AsyncMutex::new(()),
            version: AtomicU64::new(1),
            blocked: AtomicBool::new(false),
        });
        let snapshot = host.snapshot()?;
        let prepare_host = host.clone();
        let task = tokio::spawn(async move { prepare_host.prepare(larm_key, receiver).await });
        *host.prepare_task.lock().unwrap() = Some(task);
        *active = Some(host);
        Ok(snapshot)
    }

    pub async fn current(&self) -> Result<Arc<Host>, String> {
        self.active
            .lock()
            .await
            .clone()
            .ok_or("previewが開始されていません。".into())
    }

    pub async fn close(&self) -> Result<(), String> {
        let mut active = self.active.lock().await;
        if let Some(host) = active.as_ref() {
            host.close().await?;
            active.take();
        }
        Ok(())
    }
}

impl Host {
    pub fn snapshot(&self) -> Result<Snapshot, String> {
        let db = self.db.lock().map_err(|_| "DBを読み取れません。")?;
        let resource_state: String = db
            .query_row(
                "SELECT resource_state FROM conversation_preview_sessions WHERE session_id=?1",
                [&self.session_id],
                |row| row.get(0),
            )
            .map_err(|_| "資源状態を読み取れません。")?;
        let response = db
            .query_row(
                "SELECT r.response_id,r.input_id,m.content,r.action,r.frontdesk_state,
                    r.speech_state,r.public_text,r.failure_stage,r.failure_code,r.last_played_clause
             FROM conversation_preview_responses r
             JOIN conversation_messages m ON m.id=r.user_message_id
             WHERE r.session_id=?1 ORDER BY r.created_at DESC,r.rowid DESC LIMIT 1",
                [&self.session_id],
                |row| {
                    Ok(ResponseSnapshot {
                        response_id: row.get(0)?,
                        input_id: row.get(1)?,
                        input_text: row.get(2)?,
                        action: row.get(3)?,
                        frontdesk_state: row.get(4)?,
                        speech_state: row.get(5)?,
                        public_text: row.get(6)?,
                        failure_stage: row.get(7)?,
                        failure_code: row.get(8)?,
                        last_played_clause: row.get(9)?,
                    })
                },
            )
            .optional()
            .map_err(|_| "応答状態を読み取れません。")?;
        Ok(Snapshot {
            session_id: self.session_id.clone(),
            version: self.version.load(Ordering::SeqCst),
            resource_state,
            resource_phase: self.phase.lock().unwrap().clone(),
            resource_elapsed_ms: self.started_at.elapsed().as_millis() as u64,
            model: self.model.lock().unwrap().clone(),
            tts: self.config.tts.clone(),
            response,
        })
    }

    fn emit_state(&self) {
        self.version.fetch_add(1, Ordering::SeqCst);
        if let Ok(snapshot) = self.snapshot() {
            let _ = self.app.emit("preview-state", snapshot);
        }
    }

    async fn prepare(self: Arc<Self>, larm_key: String, mut cancellation: watch::Receiver<bool>) {
        let token = match credential::load_larm_token() {
            Ok(token) => token,
            Err(_) => {
                self.resource_failed("credential_missing");
                return;
            }
        };
        let (phase_tx, mut phase_rx) =
            watch::channel(saaa_larm_session::ConnectionPhase::ModelPreparing);
        let phase_host = self.clone();
        let phase_task = tokio::spawn(async move {
            while phase_rx.changed().await.is_ok() {
                *phase_host.phase.lock().unwrap() = phase_rx.borrow_and_update().as_str().into();
                phase_host.emit_state();
            }
        });
        let result = saaa_larm_session::Session::connect_with_profile_credential_key_and_phase(
            &self.config.harness_address,
            self.config.profile.clone(),
            token,
            larm_key,
            cancellation.clone(),
            Some(phase_tx),
        )
        .await;
        phase_task.abort();
        match result {
            Ok(session) => {
                if *cancellation.borrow_and_update() {
                    if session.close().await.is_err() {
                        *self.resource.lock().unwrap() = Some(session);
                        self.resource_failed("cleanup_pending");
                    }
                    return;
                }
                let summary = session.provider_summary().await;
                if !summary
                    .iter()
                    .any(|provider| provider.name == "backchannel")
                    || matches!(self.config.tts, TtsSelection::Larm { .. })
                        && !summary.iter().any(|provider| provider.name == "tts")
                {
                    self.release_failed_prepare(session, "missing_role").await;
                    return;
                }
                if let Some(provider) = summary
                    .iter()
                    .find(|provider| provider.name == "backchannel")
                {
                    *self.model.lock().unwrap() = provider.model.clone();
                }
                *self.resource.lock().unwrap() = Some(session.clone());
                let updated = self.with_db(|db| {
                    repository::set_resource_state(
                        db,
                        &self.session_id,
                        "preparing",
                        "ready",
                        Some(session.connection_id()),
                        &now(),
                    )
                });
                if updated != Ok(true) {
                    self.release_failed_prepare(session, "db_failure").await;
                    return;
                }
                *self.phase.lock().unwrap() = "ready".into();
                self.emit_state();
            }
            Err(error) => {
                if let Some(cleanup) = error.cleanup {
                    if cleanup.close().await.is_err() {
                        *self.resource.lock().unwrap() = Some(cleanup);
                        self.resource_failed("cleanup_pending");
                        return;
                    }
                }
                if *cancellation.borrow_and_update() {
                    return;
                }
                self.resource_failed("preparation_failed");
            }
        }
    }

    async fn release_failed_prepare(&self, session: Arc<saaa_larm_session::Session>, code: &str) {
        if session.close().await.is_err() {
            *self.resource.lock().unwrap() = Some(session);
            self.resource_failed("cleanup_pending");
        } else {
            self.resource.lock().unwrap().take();
            self.resource_failed(code);
        }
    }

    fn resource_failed(&self, code: &str) {
        self.blocked.store(true, Ordering::SeqCst);
        let next = if code == "cleanup_pending" {
            "cleanup_pending"
        } else {
            "failed"
        };
        let changed = self.with_db(|db| {
            let current: String = db.query_row(
                "SELECT resource_state FROM conversation_preview_sessions WHERE session_id=?1",
                [&self.session_id],
                |row| row.get(0),
            )?;
            repository::set_resource_state(db, &self.session_id, &current, next, None, &now())
        });
        if changed != Ok(false) {
            *self.phase.lock().unwrap() = code.into();
        }
        self.emit_state();
    }

    fn mark_cleanup_pending(&self) {
        let _ = self.with_db(|db| {
            let current: String = db.query_row(
                "SELECT resource_state FROM conversation_preview_sessions WHERE session_id=?1",
                [&self.session_id],
                |row| row.get(0),
            )?;
            repository::set_resource_state(
                db,
                &self.session_id,
                &current,
                "cleanup_pending",
                None,
                &now(),
            )
        });
        *self.phase.lock().unwrap() = "cleanup_pending".into();
        self.emit_state();
    }

    fn with_db<T>(
        &self,
        operation: impl FnOnce(&mut Connection) -> rusqlite::Result<T>,
    ) -> Result<T, String> {
        let mut db = self
            .db
            .lock()
            .map_err(|_| "preview DBを更新できません。".to_string())?;
        operation(&mut db).map_err(|_| "preview DBを更新できません。".to_string())
    }

    pub fn submit(
        self: &Arc<Self>,
        input_id: String,
        text: String,
    ) -> Result<SubmitReceipt, String> {
        let _admission = self
            .admission
            .lock()
            .map_err(|_| "受付状態を確認できません。")?;
        if self.blocked.load(Ordering::SeqCst) {
            return Err("実行環境は停止確認待ちのため利用できません。".into());
        }
        let input = TextInput {
            session_id: self.session_id.clone(),
            input_id,
            text,
        };
        let receipt = {
            let mut db = self.db.lock().map_err(|_| "preview DBを更新できません。")?;
            repository::accept_input(&mut db, &input, &now()).map_err(|error| -> String {
                match error {
                    repository::AcceptError::InvalidInput => {
                        "入力またはIDを確認してください。".into()
                    }
                    repository::AcceptError::SessionClosed => "sessionは終了しています。".into(),
                    repository::AcceptError::Conflict => {
                        "同じ入力IDに異なる本文を送れません。".into()
                    }
                    repository::AcceptError::Busy => "応答または音声が処理中です。".into(),
                    repository::AcceptError::NotReady => "実行環境の準備中です。".into(),
                    repository::AcceptError::Database(_) => "入力を保存できません。".into(),
                }
            })?
        };
        let result = SubmitReceipt {
            response_id: receipt.response_id.clone(),
            input_id: receipt.input_id,
            duplicate: receipt.duplicate,
        };
        if receipt.duplicate {
            return Ok(result);
        }
        let session = self
            .resource
            .lock()
            .unwrap()
            .clone()
            .ok_or("実行環境が利用できません。")?;
        let (qwen_cancel, qwen_rx) = watch::channel(false);
        let (speech_cancel, speech_rx) = watch::channel(false);
        let (speech_tx, speech_queue) = mpsc::channel(32);
        let mut speaking = Speaking::default();
        speaking.start().map_err(|_| "音声状態を開始できません。")?;
        if self.with_db(|db| {
            repository::update_speech(
                db,
                &receipt.response_id,
                0,
                "idle",
                "collecting",
                None,
                &now(),
            )
        }) != Ok(true)
        {
            let _ = self.with_db(|db| {
                repository::fail_response(
                    db,
                    &receipt.response_id,
                    "database",
                    "speech_start_commit",
                    &now(),
                )
            });
            let _ = self.with_db(|db| {
                repository::update_speech(
                    db,
                    &receipt.response_id,
                    1,
                    "stopping",
                    "stopped",
                    None,
                    &now(),
                )
            });
            self.resource_failed("db_failure");
            return Err("音声状態を保存できません。".into());
        }
        let run = Arc::new(ResponseRun {
            response_id: receipt.response_id,
            frontdesk: Mutex::new(Frontdesk::default()),
            speaking: Mutex::new(speaking),
            speech_tx,
            qwen_cancel,
            speech_cancel,
            qwen_task: Mutex::new(None),
            speech_task: Mutex::new(None),
            delta_seq: AtomicU64::new(0),
            persisted_bytes: AtomicUsize::new(0),
            speech_deadline: Instant::now() + Duration::from_secs(120),
        });
        *self.response.lock().unwrap() = Some(run.clone());
        let host = self.clone();
        let speech_run = run.clone();
        let speech_session = session.clone();
        let worker = tokio::spawn(async move {
            host.speech_worker(speech_run, speech_session, speech_queue, speech_rx)
                .await;
        });
        *run.speech_task.lock().unwrap() = Some(worker);
        let host = self.clone();
        let qwen_run = run.clone();
        let deadline = Instant::now() + Duration::from_secs(8);
        let task = tokio::spawn(async move {
            host.qwen_worker(qwen_run, session, input.text, deadline, qwen_rx)
                .await;
        });
        *run.qwen_task.lock().unwrap() = Some(task);
        self.emit_state();
        Ok(result)
    }

    async fn qwen_worker(
        self: Arc<Self>,
        run: Arc<ResponseRun>,
        session: Arc<saaa_larm_session::Session>,
        input: String,
        deadline: Instant,
        cancellation: watch::Receiver<bool>,
    ) {
        let request_id = format!("preview-{}", uuid::Uuid::new_v4().simple());
        if self
            .with_db(|db| {
                repository::record_provider_event(
                    db,
                    &run.response_id,
                    repository::ProviderEvent {
                        kind: "qwen_started",
                        request_id: Some(&request_id),
                        connection_id: Some(session.connection_id()),
                        allocation_id: None,
                        detail: None,
                        now: &now(),
                    },
                )
            })
            .is_err()
        {
            self.fail_response(&run, "database", "qwen_start_commit");
            self.resource_failed("db_failure");
            return;
        }
        let host = self.clone();
        let active = run.clone();
        let outcome = qwen::run(
            &session,
            &input,
            &request_id,
            deadline,
            cancellation,
            move |event| {
                let host = host.clone();
                let active = active.clone();
                async move { host.accept_event(&active, event) }
            },
        )
        .await;
        match outcome {
            Ok(meta) => {
                if self
                    .with_db(|db| {
                        repository::record_provider_event(
                            db,
                            &run.response_id,
                            repository::ProviderEvent {
                                kind: "qwen_finished",
                                request_id: Some(&meta.request_id),
                                connection_id: Some(&meta.connection_id),
                                allocation_id: Some(&meta.allocation_id),
                                detail: Some(&meta.model),
                                now: &now(),
                            },
                        )
                    })
                    .is_err()
                {
                    self.fail_response(&run, "database", "provider_event_commit");
                    self.resource_failed("db_failure");
                    return;
                }
                let (action, body) = {
                    let frontdesk = run.frontdesk.lock().unwrap();
                    (
                        frontdesk.action().cloned(),
                        frontdesk.public_text().to_string(),
                    )
                };
                let persisted = run.persisted_bytes.load(Ordering::SeqCst);
                let Some(final_tail) = body.get(persisted..) else {
                    self.fail_response(&run, "protocol", "public_offset_invalid");
                    return;
                };
                let completion = match action.as_ref() {
                    Some(Action::Delegate) => Ok(true),
                    Some(Action::Reply | Action::Clarify) => self.with_db(|db| {
                        repository::complete_response(db, &run.response_id, final_tail, &now())
                    }),
                    None => Err("応答本文を確定できません。".into()),
                };
                if completion == Ok(true) {
                    if matches!(action.as_ref(), Some(Action::Reply | Action::Clarify)) {
                        let _ = run.frontdesk.lock().unwrap().complete();
                    }
                    let tail_clause = {
                        let mut speaking = run.speaking.lock().unwrap();
                        if speaking.state().terminal() || speaking.state() == SpeechState::Stopping
                        {
                            Ok(None)
                        } else {
                            speaking.finish_body()
                        }
                    };
                    match tail_clause {
                        Ok(Some(clause)) => {
                            if run
                                .speech_tx
                                .try_send(SpeechCommand::Clause(clause))
                                .is_err()
                            {
                                self.speech_failed(&run, "speech_queue_full");
                            }
                        }
                        Ok(None) => {}
                        Err(_) => self.speech_failed(&run, "speech_limit"),
                    }
                    if run.speech_tx.try_send(SpeechCommand::Finish).is_err() {
                        self.speech_failed(&run, "speech_queue_full");
                        run.speech_cancel.send_replace(true);
                    }
                } else {
                    self.fail_response(&run, "database", "completion_failed");
                }
            }
            Err(failure) => {
                let _ = self.with_db(|db| {
                    repository::record_provider_event(
                        db,
                        &run.response_id,
                        repository::ProviderEvent {
                            kind: "qwen_failed",
                            request_id: failure.request_id.as_deref(),
                            connection_id: Some(session.connection_id()),
                            allocation_id: None,
                            detail: Some(&failure.code),
                            now: &now(),
                        },
                    )
                });
                self.fail_response(&run, &failure.stage, &failure.code);
                self.resource_failed("request_outcome_unknown");
            }
        }
        self.emit_state();
    }

    fn accept_event(
        &self,
        run: &Arc<ResponseRun>,
        event: Event,
    ) -> Result<(), saaa_conversation_core::contracts::Failure> {
        let reject = |code: &str| saaa_conversation_core::contracts::Failure {
            stage: "database".into(),
            code: code.into(),
            message: "応答を保存できません。".into(),
            request_id: None,
        };
        match event {
            Event::Control(action) => {
                let mut frontdesk = run.frontdesk.lock().unwrap();
                if frontdesk.state() != FrontdeskState::AwaitingControl {
                    return Err(reject("stale_control"));
                }
                let changed = self
                    .with_db(|db| {
                        repository::adopt_control(db, &run.response_id, action.clone(), &now())
                    })
                    .map_err(|_| reject("control_commit_failed"))?;
                if !changed {
                    return Err(reject("stale_control"));
                }
                frontdesk
                    .adopt_control(action.clone())
                    .map_err(|_| reject("stale_control"))?;
                drop(frontdesk);
                if action == Action::Delegate {
                    let clauses = run
                        .speaking
                        .lock()
                        .unwrap()
                        .push(saaa_conversation_core::frontdesk::DELEGATE_EXPLANATION)
                        .map_err(|_| reject("speech_limit"))?;
                    for clause in clauses {
                        if run
                            .speech_tx
                            .try_send(SpeechCommand::Clause(clause))
                            .is_err()
                        {
                            self.speech_failed(run, "speech_queue_full");
                            break;
                        }
                    }
                }
                self.emit_state();
            }
            Event::Body(text) => {
                let public_text = {
                    let mut frontdesk = run.frontdesk.lock().unwrap();
                    if frontdesk.action() == Some(&Action::Delegate) || frontdesk.state().terminal()
                    {
                        return Ok(());
                    }
                    frontdesk.append(&text).map_err(|error| match error {
                        saaa_conversation_core::frontdesk::Error::TooLong => {
                            reject("public_text_too_long")
                        }
                        _ => reject("missing_control"),
                    })?;
                    frontdesk.public_text().to_string()
                };
                let clauses = {
                    let mut speaking = run.speaking.lock().unwrap();
                    if speaking.state().terminal() || speaking.state() == SpeechState::Stopping {
                        Vec::new()
                    } else {
                        match speaking.push(&text) {
                            Ok(clauses) => clauses,
                            Err(_) => {
                                drop(speaking);
                                self.speech_failed(run, "speech_limit");
                                Vec::new()
                            }
                        }
                    }
                };
                for clause in clauses {
                    let committed = self
                        .with_db(|db| {
                            repository::checkpoint_clause(
                                db,
                                &run.response_id,
                                &clause.text,
                                clause.index,
                                &now(),
                            )
                        })
                        .map_err(|_| reject("clause_commit_failed"))?;
                    if !committed {
                        return Err(reject("clause_commit_failed"));
                    }
                    run.persisted_bytes
                        .fetch_add(clause.text.len(), Ordering::SeqCst);
                    if run
                        .speech_tx
                        .try_send(SpeechCommand::Clause(clause))
                        .is_err()
                    {
                        self.speech_failed(run, "speech_queue_full");
                        break;
                    }
                }
                let seq = run.delta_seq.fetch_add(1, Ordering::SeqCst);
                if run.frontdesk.lock().unwrap().state() != FrontdeskState::Streaming {
                    return Ok(());
                }
                let _ = self.app.emit(
                    "preview-delta",
                    serde_json::json!({
                        "responseId": run.response_id, "seq": seq,
                        "text": public_text,
                    }),
                );
                self.emit_state();
            }
        }
        Ok(())
    }

    fn fail_response(&self, run: &Arc<ResponseRun>, stage: &str, code: &str) {
        run.qwen_cancel.send_replace(true);
        run.frontdesk.lock().unwrap().fail();
        self.signal_speech_stop(run);
        let failed = self.with_db(|db| {
            let changed = repository::fail_response(db, &run.response_id, stage, code, &now())?;
            if !changed {
                repository::request_speech_stop(db, &run.response_id, &now())?;
            }
            Ok(())
        });
        if failed.is_err() {
            self.resource_failed("db_failure");
        }
        self.emit_state();
    }

    fn signal_speech_stop(&self, run: &Arc<ResponseRun>) {
        run.speaking.lock().unwrap().stop();
        run.speech_cancel.send_replace(true);
    }

    fn request_speech_stop(&self, run: &Arc<ResponseRun>) -> Result<(), String> {
        self.signal_speech_stop(run);
        self.with_db(|db| repository::request_speech_stop(db, &run.response_id, &now()))?;
        Ok(())
    }

    fn cancel_response_and_speech(&self, run: &Arc<ResponseRun>) -> Result<(), String> {
        run.qwen_cancel.send_replace(true);
        run.frontdesk.lock().unwrap().cancel();
        self.signal_speech_stop(run);
        self.with_db(|db| {
            let changed = repository::cancel_response(db, &run.response_id, &now())?;
            if !changed {
                repository::request_speech_stop(db, &run.response_id, &now())?;
            }
            Ok(())
        })
    }

    async fn speech_worker(
        self: Arc<Self>,
        run: Arc<ResponseRun>,
        session: Arc<saaa_larm_session::Session>,
        mut queue: mpsc::Receiver<SpeechCommand>,
        mut cancellation: watch::Receiver<bool>,
    ) {
        let mut finished = false;
        loop {
            let command = tokio::select! {
                biased;
                _ = await_cancel(&mut cancellation) => None,
                command = queue.recv() => command,
            };
            let Some(command) = command else {
                break;
            };
            if *cancellation.borrow() {
                break;
            }
            match command {
                SpeechCommand::Finish => {
                    finished = true;
                    break;
                }
                SpeechCommand::Clause(clause) => {
                    let next = run.speaking.lock().unwrap().begin_next();
                    if !matches!(next, Ok(Some(ref item)) if item.index == clause.index) {
                        if *cancellation.borrow() {
                            break;
                        }
                        self.speech_failed(&run, "speech_order");
                        break;
                    }
                    let generation = run.speaking.lock().unwrap().generation();
                    let changed = self.with_db(|db| {
                        repository::update_speech(
                            db,
                            &run.response_id,
                            generation,
                            "collecting",
                            "synthesizing",
                            None,
                            &now(),
                        )
                    });
                    if changed != Ok(true) {
                        if *cancellation.borrow() {
                            break;
                        }
                        self.speech_failed(&run, "speech_state_commit");
                        break;
                    }
                    self.emit_state();
                    let private_dir = match private_speech_directory(&self.data_directory) {
                        Ok(directory) => directory,
                        Err(_) => {
                            self.speech_failed(&run, "speech_artifact_unavailable");
                            break;
                        }
                    };
                    let tts_request_id = format!("preview-tts-{}", uuid::Uuid::new_v4().simple());
                    let tts_connection_id = matches!(self.config.tts, TtsSelection::Larm { .. })
                        .then_some(session.connection_id());
                    if self
                        .with_db(|db| {
                            repository::record_provider_event(
                                db,
                                &run.response_id,
                                repository::ProviderEvent {
                                    kind: "tts_started",
                                    request_id: Some(&tts_request_id),
                                    connection_id: tts_connection_id,
                                    allocation_id: None,
                                    detail: Some(&clause.index.to_string()),
                                    now: &now(),
                                },
                            )
                        })
                        .is_err()
                    {
                        self.fail_response(&run, "database", "tts_start_commit");
                        self.resource_failed("db_failure");
                        break;
                    }
                    let synthesis = speech::synthesize(
                        &self.config.tts,
                        &session,
                        &clause.text,
                        &private_dir,
                        &mut cancellation,
                        run.speech_deadline,
                        &tts_request_id,
                    )
                    .await;
                    let synthesis = match synthesis {
                        Ok(synthesis) => synthesis,
                        Err(code) => {
                            if self
                                .with_db(|db| {
                                    repository::record_provider_event(
                                        db,
                                        &run.response_id,
                                        repository::ProviderEvent {
                                            kind: "tts_failed",
                                            request_id: Some(&tts_request_id),
                                            connection_id: tts_connection_id,
                                            allocation_id: None,
                                            detail: Some(&code),
                                            now: &now(),
                                        },
                                    )
                                })
                                .is_err()
                            {
                                self.fail_response(&run, "database", "tts_failure_commit");
                                self.resource_failed("db_failure");
                                break;
                            }
                            if code == "speech_cancelled" {
                                break;
                            }
                            self.speech_failed(&run, &code);
                            break;
                        }
                    };
                    if self
                        .with_db(|db| {
                            repository::record_provider_event(
                                db,
                                &run.response_id,
                                repository::ProviderEvent {
                                    kind: "tts_finished",
                                    request_id: Some(&tts_request_id),
                                    connection_id: tts_connection_id,
                                    allocation_id: synthesis.allocation_id.as_deref(),
                                    detail: synthesis.model.as_deref(),
                                    now: &now(),
                                },
                            )
                        })
                        .is_err()
                    {
                        self.fail_response(&run, "database", "tts_finish_commit");
                        self.resource_failed("db_failure");
                        break;
                    }
                    let host = self.clone();
                    let playback_run = run.clone();
                    let index = clause.index;
                    let played = speech::play(
                        &synthesis.artifact,
                        &mut cancellation,
                        run.speech_deadline,
                        move || {
                            playback_run
                                .speaking
                                .lock()
                                .unwrap()
                                .playback_started(index, generation)
                                .map_err(|_| "stale_playback".to_string())?;
                            let changed = host.with_db(|db| {
                                repository::update_speech(
                                    db,
                                    &playback_run.response_id,
                                    generation,
                                    "synthesizing",
                                    "playing",
                                    None,
                                    &now(),
                                )
                            })?;
                            if !changed {
                                return Err("playback_state_commit".into());
                            }
                            host.emit_state();
                            Ok(())
                        },
                    )
                    .await;
                    if let Err(code) = played {
                        if code != "speech_cancelled"
                            && !((code == "stale_playback" || code == "playback_state_commit")
                                && *cancellation.borrow())
                        {
                            self.speech_failed(&run, &code);
                        }
                        break;
                    }
                    if *cancellation.borrow() {
                        break;
                    }
                    let state = {
                        let mut speaking = run.speaking.lock().unwrap();
                        speaking
                            .playback_ended(index, generation)
                            .map(|_| speaking.state())
                    };
                    let state = match state {
                        Ok(state) => state,
                        Err(_) => {
                            if *cancellation.borrow() {
                                break;
                            }
                            self.speech_failed(&run, "stale_playback");
                            break;
                        }
                    };
                    let next = if state == SpeechState::Played {
                        "played"
                    } else {
                        "collecting"
                    };
                    if self.with_db(|db| {
                        repository::update_speech(
                            db,
                            &run.response_id,
                            generation,
                            "playing",
                            next,
                            Some(index),
                            &now(),
                        )
                    }) != Ok(true)
                    {
                        if *cancellation.borrow() {
                            break;
                        }
                        self.speech_failed(&run, "playback_end_commit");
                        break;
                    }
                    self.emit_state();
                }
            }
        }
        if *cancellation.borrow() {
            let generation = run.speaking.lock().unwrap().generation();
            let _ = run.speaking.lock().unwrap().confirm_stopped();
            let _ = self.with_db(|db| {
                repository::update_speech(
                    db,
                    &run.response_id,
                    generation,
                    "stopping",
                    "stopped",
                    None,
                    &now(),
                )
            });
        } else if finished {
            let state = run.speaking.lock().unwrap().state();
            if state == SpeechState::Played {
                let generation = run.speaking.lock().unwrap().generation();
                let _ = self.with_db(|db| {
                    repository::update_speech(
                        db,
                        &run.response_id,
                        generation,
                        "collecting",
                        "played",
                        None,
                        &now(),
                    )
                });
            }
        }
        self.emit_state();
    }

    fn speech_failed(&self, run: &Arc<ResponseRun>, code: &str) {
        let uncertain_stop = matches!(
            code,
            "player_stop_unconfirmed" | "player_join_failed" | "tts_request_outcome_unknown"
        );
        let mut speaking = run.speaking.lock().unwrap();
        if speaking.state() == SpeechState::Stopping && !uncertain_stop {
            return;
        }
        speaking.fail();
        drop(speaking);
        run.speech_cancel.send_replace(true);
        let saved = self.with_db(|db| repository::fail_speech(db, &run.response_id, code, &now()));
        if saved.is_err() {
            run.qwen_cancel.send_replace(true);
            self.resource_failed("db_failure");
        }
        if code == "player_stop_unconfirmed"
            || code == "player_join_failed"
            || code == "tts_request_outcome_unknown"
        {
            self.resource_failed(code);
        }
        self.emit_state();
    }

    pub async fn stop(&self, scope: &str) -> Result<(), String> {
        let _shutdown = self.shutdown.lock().await;
        let run = self
            .response
            .lock()
            .unwrap()
            .clone()
            .ok_or("応答がありません。")?;
        let storage_saved = if scope == "response" {
            self.cancel_response_and_speech(&run).is_ok()
        } else if scope != "speech" {
            return Err("停止範囲を確認してください。".into());
        } else {
            self.request_speech_stop(&run).is_ok()
        };
        self.emit_state();
        if !join_task(&run.speech_task, Duration::from_secs(5)).await {
            self.resource_failed("stop_unconfirmed");
            return Err("音声停止を確認できません。".into());
        }
        if scope == "response" && !join_task(&run.qwen_task, Duration::from_secs(5)).await {
            self.resource_failed("request_outcome_unknown");
            return Err("遠隔応答の停止を確認できません。".into());
        }
        let terminal: bool = self
            .with_db(|db| {
                db.query_row(
                    "SELECT speech_state IN ('played','stopped','failed','interrupted')
             FROM conversation_preview_responses WHERE response_id=?1",
                    [&run.response_id],
                    |row| row.get(0),
                )
            })
            .unwrap_or(false);
        if !storage_saved || !terminal {
            self.resource_failed("db_failure");
            return Err("停止状態を保存・確認できません。".into());
        }
        Ok(())
    }

    async fn close(&self) -> Result<(), String> {
        let _shutdown = self.shutdown.lock().await;
        let active_response = {
            let _admission = self
                .admission
                .lock()
                .map_err(|_| "受付状態を確認できません。")?;
            self.blocked.store(true, Ordering::SeqCst);
            let _ = self.with_db(|db| {
                let current: String = db.query_row(
                    "SELECT resource_state FROM conversation_preview_sessions WHERE session_id=?1",
                    [&self.session_id],
                    |row| row.get(0),
                )?;
                if current == "closed" || current == "closing" {
                    return Ok(false);
                }
                repository::set_resource_state(
                    db,
                    &self.session_id,
                    &current,
                    "closing",
                    None,
                    &now(),
                )
            });
            self.resource_cancel.send_replace(true);
            let run = self.response.lock().unwrap().clone();
            if let Some(run) = &run {
                let _ = self.cancel_response_and_speech(run);
            }
            run
        };
        if let Some(run) = &active_response {
            if !join_task(&run.qwen_task, Duration::from_secs(5)).await
                || !join_task(&run.speech_task, Duration::from_secs(5)).await
            {
                self.mark_cleanup_pending();
                return Err("応答または音声の停止を確認できません。".into());
            }
        }
        let response_saved = if let Some(run) = &active_response {
            let cancellation_saved = self.cancel_response_and_speech(run).is_ok();
            let generation = run.speaking.lock().unwrap().generation();
            let final_speech = self.with_db(|db| {
                let _ = repository::update_speech(
                    db,
                    &run.response_id,
                    generation,
                    "stopping",
                    "stopped",
                    None,
                    &now(),
                )?;
                db.query_row(
                    "SELECT frontdesk_state IN
                       ('completed','unsupported','failed','cancelled','interrupted')
                       AND speech_state IN ('played','stopped','failed','interrupted')
                     FROM conversation_preview_responses WHERE response_id=?1",
                    [&run.response_id],
                    |row| row.get::<_, bool>(0),
                )
            });
            cancellation_saved && final_speech == Ok(true)
        } else {
            true
        };
        if !join_task(&self.prepare_task, Duration::from_secs(5)).await {
            self.mark_cleanup_pending();
            return Err("資源準備の停止を確認できません。".into());
        }
        let resource = { self.resource.lock().unwrap().take() };
        if let Some(session) = resource {
            if session.close().await.is_err() {
                *self.resource.lock().unwrap() = Some(session);
                self.mark_cleanup_pending();
                return Err("LARM資源の解放を確認できません。".into());
            }
        }
        let committed = self.with_db(|db| {
            let current: String = db.query_row(
                "SELECT resource_state FROM conversation_preview_sessions WHERE session_id=?1",
                [&self.session_id],
                |row| row.get(0),
            )?;
            if current == "closed" {
                return Ok(true);
            }
            repository::set_resource_state(db, &self.session_id, "closing", "closed", None, &now())
        })?;
        if !committed {
            return Err("終了状態を保存できません。".into());
        }
        self.emit_state();
        if !response_saved {
            return Err("応答終了状態を保存・確認できません。".into());
        }
        Ok(())
    }
}

fn now() -> String {
    chrono::Utc::now().to_rfc3339()
}

async fn join_task(task_slot: &Mutex<Option<JoinHandle<()>>>, wait: Duration) -> bool {
    let Some(mut task) = task_slot.lock().unwrap().take() else {
        return true;
    };
    match tokio::time::timeout(wait, &mut task).await {
        Ok(Ok(())) => true,
        Ok(Err(_)) => false,
        Err(_) => {
            *task_slot.lock().unwrap() = Some(task);
            false
        }
    }
}

async fn await_cancel(receiver: &mut watch::Receiver<bool>) {
    while !*receiver.borrow_and_update() {
        if receiver.changed().await.is_err() {
            break;
        }
    }
}

fn private_speech_directory(root: &std::path::Path) -> Result<PathBuf, String> {
    let directory = root.join("speech-artifacts");
    if !directory.exists() {
        let mut builder = std::fs::DirBuilder::new();
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        builder
            .create(&directory)
            .map_err(|_| "artifact_directory_create")?;
    }
    let metadata = std::fs::symlink_metadata(&directory).map_err(|_| "artifact_directory_stat")?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err("artifact_directory_invalid".into());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o777 != 0o700 {
            return Err("artifact_directory_permissions".into());
        }
    }
    Ok(directory)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn timeout_keeps_task_handle_for_later_cleanup() {
        let (release, wait) = tokio::sync::oneshot::channel::<()>();
        let slot = Mutex::new(Some(tokio::spawn(async move {
            let _ = wait.await;
        })));
        assert!(!join_task(&slot, Duration::from_millis(1)).await);
        assert!(slot.lock().unwrap().is_some());
        release.send(()).unwrap();
        assert!(join_task(&slot, Duration::from_secs(1)).await);
        assert!(slot.lock().unwrap().is_none());
    }
}
