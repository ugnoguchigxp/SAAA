//! One owner for submission, cancellation, history, reconcile, and artifacts.
use saaa_larm_session::media::{
    FailureKind, MediaArtifact, MediaError, MediaKind, MediaProgress, MediaResult,
};
use saaa_provider_routing::{resolve_route, AdapterKind, Purpose, ResolvedRoute};
use serde_json::Value;
use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};
use tokio::sync::{mpsc, watch, Mutex, Notify, OwnedSemaphorePermit, Semaphore};
use uuid::Uuid;

use crate::{
    contracts::{
        allowed_mime, ArtifactBytes, CancelOutcome, CancelStatus, Clock, HistoryQuery,
        MediaHostError, RemoteStop, RunHandle, RunTerminal,
    },
    ledger::CancelRecord,
    ports::{AvailabilitySource, GenerateCall, MediaBackend, MediaStore},
    validate_prompt, validate_run_id, GenerateInput, GenerateOutput,
};

struct Slot {
    cancel: watch::Sender<bool>,
    finished: bool,
    created: Instant,
}

pub struct MediaService {
    store: Arc<dyn MediaStore>,
    availability: Arc<dyn AvailabilitySource>,
    clock: Arc<dyn Clock>,
    backend: Arc<dyn MediaBackend>,
    runs: Arc<Mutex<HashMap<String, Slot>>>,
    slots: Arc<Semaphore>,
    downloads: Arc<Semaphore>,
    accepting: Arc<AtomicBool>,
    idle: Arc<Notify>,
}

impl MediaService {
    pub fn new(
        store: Arc<dyn MediaStore>,
        availability: Arc<dyn AvailabilitySource>,
        clock: Arc<dyn Clock>,
        backend: Arc<dyn MediaBackend>,
    ) -> Self {
        Self {
            store,
            availability,
            clock,
            backend,
            runs: Arc::new(Mutex::new(HashMap::new())),
            slots: Arc::new(Semaphore::new(2)),
            downloads: Arc::new(Semaphore::new(2)),
            accepting: Arc::new(AtomicBool::new(true)),
            idle: Arc::new(Notify::new()),
        }
    }

    pub fn reconcile_interrupted(&self) -> Result<(), MediaHostError> {
        self.store.reconcile_interrupted(&self.clock.now_iso())
    }

    pub fn history(&self, query: &HistoryQuery) -> Result<Vec<Value>, MediaHostError> {
        if let HistoryQuery::ByRunId(run_id) = query {
            validate_run_id(run_id).map_err(MediaHostError::invalid)?;
        }
        self.store.history(query)
    }

    pub async fn submit(&self, input: GenerateInput) -> Result<RunHandle, MediaHostError> {
        self.admit(&input.run_id, &input.prompt)?;
        if let Some(output) = self.stored_admission(&input.run_id)? {
            return Ok(ready(input.run_id, RunTerminal::Output(output)));
        }
        let route = self.route_for(input.kind)?;
        let permit = self
            .slots
            .clone()
            .try_acquire_owned()
            .map_err(|_| MediaHostError::capacity())?;
        let (cancel_tx, cancel_rx) = watch::channel(false);
        self.insert_slot(&input.run_id, cancel_tx).await?;
        if let Err(error) =
            self.store
                .reserve(&self.clock.now_iso(), &input.run_id, &input.kind, &route)
        {
            self.finish_slot(&input.run_id).await;
            if self.stored_cancelled(&input.run_id)? {
                return Ok(ready(
                    input.run_id.clone(),
                    RunTerminal::Output(cancelled_output(input.run_id)),
                ));
            }
            return Err(error);
        }
        let attempt_id = Uuid::new_v4().to_string();
        if let Err(error) = self.store.record_attempt(
            &self.clock.now_iso(),
            &input.run_id,
            &route,
            &attempt_id,
            None,
        ) {
            let _ = self.store.release_unsent(&input.run_id);
            self.finish_slot(&input.run_id).await;
            return Err(error);
        }
        self.spawn(
            input,
            route,
            None,
            permit,
            cancel_rx,
            TaskMode::Generate,
            attempt_id,
        )
    }

    pub async fn cancel(&self, run_id: &str) -> Result<CancelOutcome, MediaHostError> {
        validate_run_id(run_id).map_err(MediaHostError::invalid)?;
        let now = self.clock.now_iso();
        let live = self
            .runs
            .lock()
            .await
            .get(run_id)
            .is_some_and(|slot| !slot.finished);
        if live {
            if let Some(slot) = self.runs.lock().await.get(run_id) {
                slot.cancel.send_replace(true);
            }
            return match self.store.request_cancel(&now, run_id)? {
                CancelRecord::Missing => {
                    self.store.mark_absent_cancelled(&now, run_id)?;
                    Ok(CancelOutcome {
                        status: CancelStatus::Accepted,
                        remote_stop: RemoteStop::NotAttempted,
                    })
                }
                CancelRecord::AlreadyTerminal => Ok(CancelOutcome {
                    status: CancelStatus::AlreadyTerminal,
                    remote_stop: RemoteStop::NotAttempted,
                }),
                CancelRecord::Accepted => Ok(CancelOutcome {
                    status: CancelStatus::Accepted,
                    remote_stop: RemoteStop::NotAttempted,
                }),
            };
        }
        match self.store.request_cancel(&now, run_id)? {
            CancelRecord::Missing => {
                self.store.mark_absent_cancelled(&now, run_id)?;
                Ok(CancelOutcome {
                    status: CancelStatus::Accepted,
                    remote_stop: RemoteStop::NotAttempted,
                })
            }
            CancelRecord::AlreadyTerminal => Ok(CancelOutcome {
                status: CancelStatus::AlreadyTerminal,
                remote_stop: RemoteStop::NotAttempted,
            }),
            CancelRecord::Accepted => self.finish_remote_cancel(run_id).await,
        }
    }

    pub async fn reconcile(&self, run_id: &str) -> Result<RunHandle, MediaHostError> {
        validate_run_id(run_id).map_err(MediaHostError::invalid)?;
        let stored = self
            .store
            .get(run_id)?
            .ok_or_else(|| MediaHostError::not_found("生成の記録がありません"))?;
        let status = stored["status"].as_str().unwrap_or("");
        if matches!(status, "accepted" | "cancelled" | "failed") {
            return Ok(ready(
                run_id.to_string(),
                RunTerminal::Output(terminal_from_stored(run_id, &stored)?),
            ));
        }
        let route: ResolvedRoute = serde_json::from_value(stored["route"].clone())
            .map_err(|_| MediaHostError::storage("生成の設定記録が不正です"))?;
        let kind: MediaKind = serde_json::from_value(stored["kind"].clone())
            .map_err(|_| MediaHostError::invalid("生成の種類が不正です"))?;
        let job = stored["jobId"]
            .as_str()
            .filter(|value| !value.is_empty())
            .map(str::to_string);
        let synchronous_image =
            kind == MediaKind::Image && route.adapter_kind != AdapterKind::ReplicateMedia;
        let recover_unknown = status == "unknown" && job.is_some() && !synchronous_image;
        if status == "unknown" && !recover_unknown {
            return Ok(ready(
                run_id.to_string(),
                RunTerminal::Output(terminal_from_stored(run_id, &stored)?),
            ));
        }
        if self
            .runs
            .lock()
            .await
            .get(run_id)
            .is_some_and(|slot| !slot.finished)
        {
            return Err(MediaHostError::conflict("生成の処理は進行中です"));
        }
        if !synchronous_image && job.is_none() {
            return Err(MediaHostError::conflict(
                "送信結果が不明で処理IDを取得できませんでした。再送せず、サービス側の履歴を確認してください。",
            ));
        }
        let permit = self
            .slots
            .clone()
            .try_acquire_owned()
            .map_err(|_| MediaHostError::new("capacity", "他の生成の完了を待ってください。"))?;
        let (cancel_tx, cancel_rx) = watch::channel(false);
        self.insert_slot(run_id, cancel_tx).await?;
        let input = GenerateInput {
            run_id: run_id.to_string(),
            kind,
            prompt: String::new(),
        };
        let attempt_id = Uuid::new_v4().to_string();
        if let Err(error) =
            self.store
                .record_attempt(&self.clock.now_iso(), run_id, &route, &attempt_id, None)
        {
            self.finish_slot(run_id).await;
            return Err(error);
        }
        self.spawn(
            input,
            route,
            job,
            permit,
            cancel_rx,
            if recover_unknown {
                TaskMode::RecoverUnknown
            } else {
                TaskMode::Reconcile
            },
            attempt_id,
        )
    }

    pub async fn artifact(
        &self,
        run_id: &str,
        index: usize,
    ) -> Result<ArtifactBytes, MediaHostError> {
        validate_run_id(run_id).map_err(MediaHostError::invalid)?;
        if let Some(bytes) = self.store.cached(run_id, index)? {
            return Ok(bytes);
        }
        let stored = self
            .store
            .get(run_id)?
            .ok_or_else(|| MediaHostError::not_found("成果物の参照期限が切れました。"))?;
        if stored["route"]["adapterKind"] == "replicate-media" {
            return Err(MediaHostError::not_found(
                "保存済み成果物がありません。生成し直さず、進行状況の照会で取得してください。",
            ));
        }
        if stored["status"] != "accepted" {
            return Err(MediaHostError::conflict("生成の完了は確認できていません"));
        }
        let route: ResolvedRoute = serde_json::from_value(stored["route"].clone())
            .map_err(|_| MediaHostError::storage("生成設定の記録が不正です"))?;
        let kind: MediaKind = serde_json::from_value(stored["kind"].clone())
            .map_err(|_| MediaHostError::invalid("生成の種類が不正です"))?;
        let artifact: MediaArtifact =
            serde_json::from_value(stored["result"]["artifacts"][index].clone())
                .map_err(|_| MediaHostError::not_found("成果物が見つかりません。"))?;
        let mime = artifact.mime_type.clone();
        if !allowed_mime(&mime) {
            return Err(MediaHostError::invalid("成果物の形式を確認できません"));
        }
        let _permit = self
            .downloads
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| MediaHostError::storage("成果物の取得を開始できません。"))?;
        let bytes = self
            .backend
            .fetch_artifact(route, kind, artifact)
            .await
            .map_err(|error| {
                MediaHostError::storage(format!(
                    "成果物を取得できませんでした（{}）。生成し直さず、取得を再試行できます。",
                    error.code
                ))
            })?;
        self.store.cache(run_id, index, &bytes)?;
        Ok(ArtifactBytes {
            bytes,
            mime_type: mime,
        })
    }

    pub async fn shutdown(&self, grace: Duration) -> Result<(), MediaHostError> {
        self.accepting.store(false, Ordering::SeqCst);
        let ids: Vec<String> = self
            .runs
            .lock()
            .await
            .iter()
            .filter(|(_, slot)| !slot.finished)
            .map(|(id, _)| id.clone())
            .collect();
        for id in ids {
            let _ = self.cancel(&id).await;
        }
        let sleep = self.clock.sleep(grace);
        tokio::pin!(sleep);
        loop {
            if !self.has_unfinished().await {
                break;
            }
            tokio::select! {
                _ = &mut sleep => break,
                _ = self.idle.notified() => {}
            }
        }
        self.store.reconcile_interrupted(&self.clock.now_iso())
    }

    fn admit(&self, run_id: &str, prompt: &str) -> Result<(), MediaHostError> {
        if !self.accepting.load(Ordering::SeqCst) {
            return Err(MediaHostError::conflict("生成の受付を停止しています。"));
        }
        validate_run_id(run_id).map_err(MediaHostError::invalid)?;
        validate_prompt(prompt).map_err(MediaHostError::invalid)?;
        Ok(())
    }

    fn stored_admission(&self, run_id: &str) -> Result<Option<GenerateOutput>, MediaHostError> {
        let Some(stored) = self.store.get(run_id)? else {
            return Ok(None);
        };
        if stored["status"] == "cancelled" {
            return Ok(Some(cancelled_output(run_id.to_string())));
        }
        Err(MediaHostError::duplicate(
            "この生成要求は記録済みです。生成し直さず進行状況を確認してください。",
        ))
    }

    fn stored_cancelled(&self, run_id: &str) -> Result<bool, MediaHostError> {
        Ok(self
            .store
            .get(run_id)?
            .is_some_and(|stored| stored["status"] == "cancelled"))
    }

    fn route_for(&self, kind: MediaKind) -> Result<ResolvedRoute, MediaHostError> {
        let snapshot = self.store.load_registry()?;
        resolve_route(&snapshot, purpose(kind), self.availability.larm()).map_err(|error| {
            MediaHostError::route(
                error
                    .user_message()
                    .map(str::to_string)
                    .unwrap_or_else(|| format!("生成サービスの設定を確認してください: {error:?}")),
            )
        })
    }

    async fn insert_slot(
        &self,
        run_id: &str,
        cancel: watch::Sender<bool>,
    ) -> Result<(), MediaHostError> {
        let mut runs = self.runs.lock().await;
        if runs.contains_key(run_id) {
            return Err(MediaHostError::duplicate(
                "この生成要求は送信済みです。同じ要求を再送できません。",
            ));
        }
        trim(&mut runs);
        runs.insert(
            run_id.to_string(),
            Slot {
                cancel,
                finished: false,
                created: Instant::now(),
            },
        );
        Ok(())
    }

    async fn finish_slot(&self, run_id: &str) {
        if let Some(slot) = self.runs.lock().await.get_mut(run_id) {
            slot.finished = true;
        }
        self.idle.notify_waiters();
    }

    async fn has_unfinished(&self) -> bool {
        self.runs.lock().await.values().any(|slot| !slot.finished)
    }

    #[allow(clippy::too_many_arguments)]
    fn spawn(
        &self,
        input: GenerateInput,
        route: ResolvedRoute,
        resume: Option<String>,
        permit: OwnedSemaphorePermit,
        cancel: watch::Receiver<bool>,
        mode: TaskMode,
        attempt_id: String,
    ) -> Result<RunHandle, MediaHostError> {
        let (progress_tx, progress_rx) = mpsc::channel(64);
        let (terminal_tx, terminal_rx) = watch::channel(None);
        let handle = RunHandle::new(input.run_id.clone(), progress_rx, terminal_rx);
        let store = self.store.clone();
        let clock = self.clock.clone();
        let backend = self.backend.clone();
        let runs = self.runs.clone();
        let idle = self.idle.clone();
        tokio::spawn(async move {
            execute(
                store,
                clock,
                backend,
                runs,
                idle,
                input,
                route,
                resume,
                permit,
                cancel,
                progress_tx,
                terminal_tx,
                mode,
                attempt_id,
            )
            .await;
        });
        Ok(handle)
    }

    async fn finish_remote_cancel(&self, run_id: &str) -> Result<CancelOutcome, MediaHostError> {
        let stored = self
            .store
            .get(run_id)?
            .ok_or_else(|| MediaHostError::not_found("生成の記録がありません"))?;
        let Some(job) = stored["jobId"].as_str().map(str::to_string) else {
            let _ = self
                .store
                .record_phase(&self.clock.now_iso(), run_id, "unknown", None);
            return Err(MediaHostError::conflict(
                "待機の中止を記録しましたが、送信結果が不明で遠隔の停止は確認できません。",
            ));
        };
        let route: ResolvedRoute = serde_json::from_value(stored["route"].clone())
            .map_err(|_| MediaHostError::storage("生成の設定記録が不正です"))?;
        let kind = serde_json::from_value(stored["kind"].clone()).unwrap_or(MediaKind::Music);
        let mut error = self
            .backend
            .cancel_remote(route.clone(), kind, job.clone())
            .await;
        let confirmed = crate::backend::remote_cancel_confirmed(&error);
        if !confirmed {
            error.may_have_generated = true;
            error.job_id = Some(job);
        }
        self.store
            .finish(&self.clock.now_iso(), run_id, &route, &Err(error))?;
        Ok(CancelOutcome {
            status: CancelStatus::Accepted,
            remote_stop: if confirmed {
                RemoteStop::Confirmed
            } else {
                RemoteStop::Unconfirmed
            },
        })
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum TaskMode {
    Generate,
    Reconcile,
    RecoverUnknown,
}

#[allow(clippy::too_many_arguments)]
async fn execute(
    store: Arc<dyn MediaStore>,
    clock: Arc<dyn Clock>,
    backend: Arc<dyn MediaBackend>,
    runs: Arc<Mutex<HashMap<String, Slot>>>,
    idle: Arc<Notify>,
    input: GenerateInput,
    route: ResolvedRoute,
    resume: Option<String>,
    permit: OwnedSemaphorePermit,
    cancel: watch::Receiver<bool>,
    progress_tx: mpsc::Sender<MediaProgress>,
    terminal_tx: watch::Sender<Option<RunTerminal>>,
    mode: TaskMode,
    attempt_id: String,
) {
    let reconcile = mode != TaskMode::Generate;
    let _permit = permit;
    let run_id = input.run_id.clone();
    let progress = progress_fn(
        store.clone(),
        clock.clone(),
        run_id.clone(),
        progress_tx.clone(),
    );
    let publish = |terminal: RunTerminal| {
        let _ = terminal_tx.send(Some(terminal));
    };
    if *cancel.borrow() && !reconcile {
        let output = cancelled_output(run_id.clone());
        let _ = store.finish(
            &clock.now_iso(),
            &run_id,
            &route,
            &Err(output.error.clone().expect("cancelled")),
        );
        publish(stored_terminal(&store, &run_id).unwrap_or(RunTerminal::Output(output)));
        mark_finished(&runs, &idle, &run_id).await;
        return;
    }
    progress(MediaProgress {
        phase: "discovering".into(),
        job_id: resume.clone(),
        progress: None,
    });
    let call = GenerateCall {
        run_id: run_id.clone(),
        kind: input.kind,
        prompt: input.prompt,
        route: route.clone(),
        cancel,
        progress: Arc::new(progress),
        resume: resume.clone(),
    };
    let image_without_job = reconcile
        && route.adapter_kind != AdapterKind::ReplicateMedia
        && input.kind != MediaKind::Music;
    let result = if image_without_job {
        Err(MediaError {
            kind: FailureKind::OutcomeUnknown,
            code: "synchronous_image_has_no_job".into(),
            retryable: false,
            may_have_generated: true,
            job_id: resume.clone(),
        })
    } else if reconcile || route.adapter_kind == AdapterKind::ReplicateMedia {
        let future = if reconcile {
            backend.reconcile(call)
        } else {
            backend.generate(call)
        };
        future.await
    } else {
        let future = backend.generate(call);
        tokio::select! {
            _ = clock.sleep(Duration::from_millis(route.timeout_ms)) => Err(deadline(&store, &run_id)),
            result = future => result,
        }
    };
    if let Err(error) = store.record_attempt(
        &clock.now_iso(),
        &run_id,
        &route,
        &attempt_id,
        Some(result.is_ok()),
    ) {
        let _ = store.record_phase(
            &clock.now_iso(),
            &run_id,
            "unknown",
            result.as_ref().ok().and_then(|item| item.job_id.as_deref()),
        );
        publish(RunTerminal::HostFailure(error));
        mark_finished(&runs, &idle, &run_id).await;
        return;
    }
    let persisted = if mode == TaskMode::RecoverUnknown {
        store.finish_query(&clock.now_iso(), &run_id, &route, &result)
    } else {
        store.finish(&clock.now_iso(), &run_id, &route, &result)
    };
    if let Err(error) = persisted {
        let _ = store.record_phase(
            &clock.now_iso(),
            &run_id,
            "unknown",
            result.as_ref().ok().and_then(|item| item.job_id.as_deref()),
        );
        publish(RunTerminal::HostFailure(error));
        mark_finished(&runs, &idle, &run_id).await;
        return;
    }
    publish(stored_terminal(&store, &run_id).unwrap_or_else(RunTerminal::HostFailure));
    mark_finished(&runs, &idle, &run_id).await;
}

fn stored_terminal(
    store: &Arc<dyn MediaStore>,
    run_id: &str,
) -> Result<RunTerminal, MediaHostError> {
    let stored = store
        .get(run_id)?
        .ok_or_else(|| MediaHostError::storage("生成結果の記録がありません"))?;
    Ok(RunTerminal::Output(terminal_from_stored(run_id, &stored)?))
}

fn progress_fn(
    store: Arc<dyn MediaStore>,
    clock: Arc<dyn Clock>,
    run_id: String,
    progress_tx: mpsc::Sender<MediaProgress>,
) -> impl Fn(MediaProgress) + Send + Sync {
    move |event: MediaProgress| {
        let _ = store.record_phase(
            &clock.now_iso(),
            &run_id,
            &event.phase,
            event.job_id.as_deref(),
        );
        let _ = progress_tx.try_send(event);
    }
}

fn deadline(store: &Arc<dyn MediaStore>, run_id: &str) -> MediaError {
    let job = store
        .get(run_id)
        .ok()
        .flatten()
        .and_then(|value| value["jobId"].as_str().map(str::to_string));
    MediaError {
        kind: FailureKind::OutcomeUnknown,
        code: "generation_deadline_exceeded".into(),
        retryable: false,
        may_have_generated: true,
        job_id: job,
    }
}

async fn mark_finished(runs: &Mutex<HashMap<String, Slot>>, idle: &Notify, run_id: &str) {
    if let Some(slot) = runs.lock().await.get_mut(run_id) {
        slot.finished = true;
    }
    idle.notify_waiters();
}

fn purpose(kind: MediaKind) -> Purpose {
    match kind {
        MediaKind::Image => Purpose::MediaImageGenerate,
        MediaKind::Music => Purpose::MediaMusicGenerate,
    }
}

fn cancelled_output(run_id: String) -> GenerateOutput {
    GenerateOutput {
        run_id,
        result: None,
        error: Some(MediaError {
            kind: FailureKind::Cancelled,
            code: "cancelled_before_submission".into(),
            retryable: false,
            may_have_generated: false,
            job_id: None,
        }),
    }
}

fn terminal_from_stored(run_id: &str, stored: &Value) -> Result<GenerateOutput, MediaHostError> {
    let output = output_from_stored(run_id, stored);
    if output.result.is_some() || output.error.is_some() {
        return Ok(output);
    }
    if stored["status"] == "accepted" {
        return Err(MediaHostError::storage("生成の記録が不正です"));
    }
    Ok(GenerateOutput {
        run_id: run_id.to_string(),
        result: None,
        error: Some(MediaError {
            kind: FailureKind::OutcomeUnknown,
            code: "reconcile_interrupted".into(),
            retryable: false,
            may_have_generated: true,
            job_id: stored["jobId"].as_str().map(str::to_string),
        }),
    })
}

fn output_from_stored(run_id: &str, stored: &Value) -> GenerateOutput {
    GenerateOutput {
        run_id: run_id.to_string(),
        result: stored.get("result").and_then(media_result),
        error: stored.get("error").and_then(media_error),
    }
}

fn media_result(value: &Value) -> Option<MediaResult> {
    if value.is_null() {
        return None;
    }
    Some(MediaResult {
        kind: serde_json::from_value(value["kind"].clone()).ok()?,
        model: value["model"].as_str()?.to_string(),
        job_id: value["jobId"].as_str().map(str::to_string),
        artifacts: serde_json::from_value(value["artifacts"].clone()).ok()?,
    })
}

fn media_error(value: &Value) -> Option<MediaError> {
    if value.is_null() {
        return None;
    }
    let kind = match value["kind"].as_str()? {
        "discovery" => FailureKind::Discovery,
        "conflict" => FailureKind::Conflict,
        "startupFailed" => FailureKind::StartupFailed,
        "generationFailed" => FailureKind::GenerationFailed,
        "timeout" => FailureKind::Timeout,
        "cancelled" => FailureKind::Cancelled,
        "outcomeUnknown" => FailureKind::OutcomeUnknown,
        "artifactFailed" => FailureKind::ArtifactFailed,
        _ => FailureKind::Protocol,
    };
    Some(MediaError {
        kind,
        code: value["code"].as_str()?.to_string(),
        retryable: value["retryable"].as_bool().unwrap_or(false),
        may_have_generated: value["mayHaveGenerated"].as_bool().unwrap_or(false),
        job_id: value["jobId"].as_str().map(str::to_string),
    })
}

fn ready(run_id: String, terminal: RunTerminal) -> RunHandle {
    let (_progress_tx, progress_rx) = mpsc::channel(1);
    let (_terminal_tx, terminal_rx) = watch::channel(Some(terminal));
    RunHandle::new(run_id, progress_rx, terminal_rx)
}

fn trim(entries: &mut HashMap<String, Slot>) {
    if entries.len() >= 16 {
        let oldest = entries
            .iter()
            .filter(|(_, entry)| entry.finished)
            .min_by_key(|(_, entry)| entry.created)
            .map(|(id, _)| id.clone());
        if let Some(oldest) = oldest {
            entries.remove(&oldest);
        }
    }
}
