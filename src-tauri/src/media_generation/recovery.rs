//! Recovery only polls a stored remote ID. It never creates another prediction.
use super::*;
use rusqlite::params;
use saaa_larm_session::media::FailureKind;

#[tauri::command]
pub(crate) fn list_media_generations(
    state: tauri::State<'_, AppState>,
) -> Result<Vec<Value>, String> {
    state.sqlite_writer.write(|db| ledger::initialize(db))?;
    state.sqlite_readers.read(|db| {
        let mut statement=db.prepare("SELECT run_id,kind,route_json,state,remote_id,result_json,error_json,updated_at FROM purpose_media_operations ORDER BY updated_at DESC LIMIT 20").map_err(crate::database_error)?;
        let rows=statement.query_map([],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?,r.get::<_,String>(3)?,r.get::<_,Option<String>>(4)?,r.get::<_,Option<String>>(5)?,r.get::<_,Option<String>>(6)?,r.get::<_,String>(7)?))).map_err(crate::database_error)?;
        rows.map(|row| {let (run,kind,route,status,job,result,error,at)=row.map_err(crate::database_error)?;
            let route:Value=serde_json::from_str(&route).map_err(|e|e.to_string())?;
            Ok(json!({"runId":run,"kind":serde_json::from_str::<Value>(&kind).map_err(|e|e.to_string())?,"connectionLabel":route["connectionLabel"],"model":route["model"],"status":status,"jobId":job,"result":result.map(|v|serde_json::from_str::<Value>(&v)).transpose().map_err(|e|e.to_string())?,"error":error.map(|v|serde_json::from_str::<Value>(&v)).transpose().map_err(|e|e.to_string())?,"updatedAt":at}))
        }).collect()
    })
}

#[tauri::command]
pub(crate) async fn reconcile_media_generation(
    state: tauri::State<'_, AppState>,
    run_id: String,
    on_progress: tauri::ipc::Channel<MediaProgress>,
) -> Result<Value, String> {
    validate_id(&run_id)?;
    let stored = ledger::get(&state, &run_id)?.ok_or("生成の記録がありません")?;
    if stored["status"] == "accepted" {
        return Ok(json!({"runId":run_id,"result":stored["result"],"error":null}));
    }
    let id=stored["jobId"].as_str().ok_or("送信結果が不明で処理IDを取得できませんでした。再送せず、サービス側の履歴を確認してください。")?;
    let route: ResolvedRoute =
        serde_json::from_value(stored["route"].clone()).map_err(|_| "生成の設定記録が不正です")?;
    let kind: MediaKind =
        serde_json::from_value(stored["kind"].clone()).map_err(|_| "生成の種類が不正です")?;
    let (cancel, receiver) = watch::channel(false);
    {
        let mut entries = runs().lock().await;
        if entries.get(&run_id).is_some_and(|e| !e.finished) {
            return Err("生成の処理は進行中です".into());
        }
        if entries.values().filter(|e| !e.finished).count() >= 2 {
            return Err("他の生成の完了を待ってください".into());
        }
        entries.insert(
            run_id.clone(),
            Entry {
                cancel,
                client: None,
                result: None,
                finished: false,
                created: std::time::Instant::now(),
            },
        );
    }
    let attempt_id = uuid::Uuid::new_v4().to_string();
    if let Err(error) = state.sqlite_writer.write(|db| {
        crate::providers::service_registry::operations::attempt(
            db,
            &run_id,
            &route,
            &attempt_id,
            None,
        )
    }) {
        if let Some(entry) = runs().lock().await.get_mut(&run_id) {
            entry.finished = true;
        }
        return Err(error);
    }
    let result = if route.adapter_kind == AdapterKind::ReplicateMedia {
        replicate::generate(
            &state,
            &run_id,
            kind,
            "",
            &route,
            receiver,
            &on_progress,
            Some(id),
        )
        .await
    } else if kind == MediaKind::Music {
        async {
            let client = larm_client(&state, &route, kind).await?;
            runs()
                .lock()
                .await
                .get_mut(&run_id)
                .expect("recovery entry")
                .client = Some(client.clone());
            client
                .resume_music_job(id, receiver, &|event| {
                    let _ = ledger::phase(&state, &run_id, &event.phase, event.job_id.as_deref());
                    let _ = on_progress.send(event);
                })
                .await
        }
        .await
    } else {
        Err(MediaError {
            kind: FailureKind::OutcomeUnknown,
            code: "synchronous_image_has_no_job".into(),
            retryable: false,
            may_have_generated: true,
            job_id: Some(id.into()),
        })
    };
    let persisted = state
        .sqlite_writer
        .write(|db| {
            crate::providers::service_registry::operations::attempt(
                db,
                &run_id,
                &route,
                &attempt_id,
                Some(result.is_ok()),
            )
        })
        .and_then(|_| ledger::finish(&state, &run_id, &route, &result));
    let mut entries = runs().lock().await;
    let entry = entries.get_mut(&run_id).expect("recovery entry");
    entry.finished = true;
    if let Err(error) = persisted {
        let _ = ledger::phase(&state, &run_id, "unknown", Some(id));
        return Err(error);
    }
    match result {
        Ok(result) => {
            entry.result = Some(result.clone());
            Ok(json!({"runId":run_id,"result":result,"error":null}))
        }
        Err(error) => Ok(json!({"runId":run_id,"result":null,"error":error})),
    }
}

pub(super) async fn larm_client(
    state: &AppState,
    route: &ResolvedRoute,
    kind: MediaKind,
) -> Result<Arc<MediaClient>, MediaError> {
    let credential = crate::providers::dynamic_lan::credential::load().map_err(|_| MediaError {
        kind: FailureKind::Discovery,
        code: "credential_missing".into(),
        retryable: false,
        may_have_generated: false,
        job_id: None,
    })?;
    let client = MediaClient::discover(&route.endpoint, credential.token().into(), kind).await?;
    let readers = state.sqlite_readers.clone();
    let pinned = route.clone();
    Ok(Arc::new(client.with_request_guard(Arc::new(move || {
        readers
            .read(|db| crate::providers::service_registry::validate_active(db, &pinned))
            .map_err(|_| MediaError {
                kind: FailureKind::Cancelled,
                code: "route_revoked".into(),
                retryable: false,
                may_have_generated: false,
                job_id: None,
            })
    }))))
}

pub(super) async fn cancel_stored(state: &AppState, run: &str) -> Result<(), String> {
    let stored = ledger::get(state, run)?;
    let Some(stored) = stored else {
        return state.sqlite_writer.write(|db| {db.execute("INSERT OR IGNORE INTO purpose_media_operations(run_id,kind,route_json,state,updated_at) VALUES(?1,'null','null','cancelled',?2)",params![run,crate::now_iso()]).map_err(crate::database_error)?;Ok(())});
    };
    if stored["status"] == "accepted" || stored["status"] == "cancelled" {
        return Ok(());
    }
    if runs().lock().await.get(run).is_some_and(|e| !e.finished) {
        ledger::phase(state, run, "cancel_requested", stored["jobId"].as_str())?;
        return Ok(());
    }
    let Some(id) = stored["jobId"].as_str() else {
        ledger::phase(state, run, "unknown", None)?;
        return Err(
            "待機の中止を記録しましたが、送信結果が不明で遠隔の停止は確認できません。".into(),
        );
    };
    let route: ResolvedRoute =
        serde_json::from_value(stored["route"].clone()).map_err(|_| "生成の設定記録が不正です")?;
    let error = if route.adapter_kind == AdapterKind::ReplicateMedia {
        replicate::cancel_existing(state, &route, id).await
    } else {
        match larm_client(state, &route, MediaKind::Music).await {
            Ok(client) => client.cancel_music_job(id).await,
            Err(error) => error,
        }
    };
    let mut error = error;
    let confirmed = matches!(
        error.code.as_str(),
        "remote_cancel_confirmed" | "music_cancelled"
    ) && !error.may_have_generated;
    if !confirmed {
        error.may_have_generated = true;
        error.job_id = Some(id.into());
    }
    ledger::finish(state, run, &route, &Err(error))?;
    if confirmed {
        Ok(())
    } else {
        Err("遠隔の停止を確認できませんでした。生成し直さず、進行状況を照会してください。".into())
    }
}
