//! Submission ownership and completion boundary.
use super::*;

#[tauri::command]
pub(crate) async fn generate_media(
    state: tauri::State<'_, AppState>,
    input: GenerateInput,
    on_progress: tauri::ipc::Channel<MediaProgress>,
) -> Result<GenerateOutput, String> {
    validate_id(&input.run_id)?;
    if input.prompt.trim().is_empty() || input.prompt.len() > 16_384 {
        return Err("生成内容は1〜16384バイトで入力してください。".into());
    }
    if let Some(stored) = ledger::get(&state, &input.run_id)? {
        if stored["status"] == "cancelled" {
            return Ok(GenerateOutput {
                run_id: input.run_id,
                result: None,
                error: Some(MediaError {
                    kind: saaa_larm_session::media::FailureKind::Cancelled,
                    code: "cancelled_before_submission".into(),
                    retryable: false,
                    may_have_generated: false,
                    job_id: None,
                }),
            });
        }
        return Err("この生成要求は記録済みです。生成し直さず進行状況を確認してください。".into());
    }
    let availability = crate::providers::service_registry::LocalAvailability::of(&state);
    let route = state.sqlite_readers.read(|db| {
        let loaded = persistence::service_registry_store::load_registry(db)?;
        crate::providers::service_registry::resolve_route(
            &loaded.snapshot,
            match input.kind {
                MediaKind::Image => Purpose::MediaImageGenerate,
                MediaKind::Music => Purpose::MediaMusicGenerate,
            },
            availability,
        )
        .map_err(|e| {
            e.user_message()
                .map(str::to_string)
                .unwrap_or_else(|| format!("生成サービスの設定を確認してください: {e:?}"))
        })
    })?;
    let (cancel, receiver) = watch::channel(false);
    {
        let mut entries = runs().lock().await;
        if let Some(entry) = entries.get(&input.run_id) {
            if *entry.cancel.borrow() && entry.client.is_none() && entry.finished {
                return Ok(GenerateOutput {
                    run_id: input.run_id,
                    result: None,
                    error: Some(MediaError {
                        kind: saaa_larm_session::media::FailureKind::Cancelled,
                        code: "cancelled_before_submission".into(),
                        retryable: false,
                        may_have_generated: false,
                        job_id: None,
                    }),
                });
            }
            return Err("この生成要求は送信済みです。同じ要求を再送できません。".into());
        }
        if entries.values().filter(|entry| !entry.finished).count() >= 2 {
            return Err("生成要求を処理中です。完了までお待ちください。".into());
        }
        trim(&mut entries);
        entries.insert(
            input.run_id.clone(),
            Entry {
                cancel,
                client: None,
                result: None,
                finished: false,
                created: std::time::Instant::now(),
            },
        );
    }
    if let Err(error) = ledger::reserve(&state, &input.run_id, input.kind, &route) {
        runs().lock().await.remove(&input.run_id);
        return Err(error);
    }
    let _ = on_progress.send(MediaProgress {
        phase: "discovering".into(),
        job_id: None,
        progress: None,
    });
    let attempt_id = uuid::Uuid::new_v4().to_string();
    if let Err(error) = state.sqlite_writer.write(|db| {
        crate::providers::service_registry::operations::attempt(
            db,
            &input.run_id,
            &route,
            &attempt_id,
            None,
        )
    }) {
        if let Some(entry) = runs().lock().await.get_mut(&input.run_id) {
            entry.finished = true;
        }
        return Err(error);
    }
    let started = tokio::time::Instant::now();
    let mut result = if route.adapter_kind == AdapterKind::ReplicateMedia {
        replicate::generate(
            &state,
            &input.run_id,
            input.kind,
            &input.prompt,
            &route,
            receiver,
            &on_progress,
            None,
        )
        .await
    } else {
        tokio::select! {
            _=tokio::time::sleep_until(started+std::time::Duration::from_millis(route.timeout_ms))=>Err(MediaError{kind:saaa_larm_session::media::FailureKind::OutcomeUnknown,code:"generation_deadline_exceeded".into(),retryable:false,may_have_generated:true,job_id:ledger::get(&state,&input.run_id).ok().flatten().and_then(|v|v["jobId"].as_str().map(str::to_string))}),
            result=async {
            let credential=crate::providers::dynamic_lan::credential::load().map_err(|_| MediaError{kind:saaa_larm_session::media::FailureKind::Discovery,code:"credential_missing".into(),retryable:false,may_have_generated:false,job_id:None})?;
            let mut client=MediaClient::discover(&route.endpoint,credential.token().into(),input.kind).await?;
            // The single POST includes cold model loading, generation and worker shutdown.
            client.limits.submission=std::time::Duration::from_millis(route.timeout_ms.saturating_sub(started.elapsed().as_millis() as u64));
            client.limits.job=std::time::Duration::from_millis(route.timeout_ms.saturating_sub(started.elapsed().as_millis() as u64));
            let readers=state.sqlite_readers.clone();let pinned=route.clone();
            let client=Arc::new(client.with_request_guard(Arc::new(move||readers.read(|db|crate::providers::service_registry::validate_active(db,&pinned)).map_err(|_|MediaError{kind:saaa_larm_session::media::FailureKind::Cancelled,code:"route_revoked".into(),retryable:false,may_have_generated:false,job_id:None}))));
            runs()
                .lock()
                .await
                .get_mut(&input.run_id)
                .expect("registered media run")
                .client = Some(client.clone());
            client
                .generate(&input.prompt, receiver, &|event| {
                    let _ = ledger::phase(&state,&input.run_id,&event.phase,event.job_id.as_deref());
                    let _ = on_progress.send(event);
                })
                .await
        }=>result,
        }
    };
    if route.adapter_kind != AdapterKind::ReplicateMedia {
        if let Ok(output) = &result {
            let client = runs()
                .lock()
                .await
                .get(&input.run_id)
                .and_then(|e| e.client.clone())
                .ok_or("生成サービスを確認できません")?;
            let mut total = 0usize;
            for (index, artifact) in output.artifacts.iter().enumerate() {
                let deadline = started + std::time::Duration::from_millis(route.timeout_ms);
                let downloaded = tokio::select! {
                    _=tokio::time::sleep_until(deadline)=>Err(MediaError{kind:saaa_larm_session::media::FailureKind::OutcomeUnknown,code:"artifact_deadline_exceeded".into(),retryable:false,may_have_generated:true,job_id:output.job_id.clone()}),
                    value=client.artifact_bytes(artifact)=>value,
                };
                match downloaded {
                    Ok(bytes) => {
                        total += bytes.len();
                        if total > 64 * 1024 * 1024
                            || ledger::cache(&state, &input.run_id, index, &bytes).is_err()
                        {
                            result = Err(MediaError {
                                kind: saaa_larm_session::media::FailureKind::ArtifactFailed,
                                code: "artifact_storage_failed".into(),
                                retryable: false,
                                may_have_generated: true,
                                job_id: output.job_id.clone(),
                            });
                            break;
                        }
                    }
                    Err(mut error) => {
                        error.may_have_generated = true;
                        error.job_id = output.job_id.clone();
                        result = Err(error);
                        break;
                    }
                }
            }
        }
    }
    if let Err(error) = state.sqlite_writer.write(|db| {
        crate::providers::service_registry::operations::attempt(
            db,
            &input.run_id,
            &route,
            &attempt_id,
            Some(result.is_ok()),
        )
    }) {
        if let Some(entry) = runs().lock().await.get_mut(&input.run_id) {
            entry.finished = true;
        }
        let _ = ledger::phase(
            &state,
            &input.run_id,
            "unknown",
            result.as_ref().ok().and_then(|r| r.job_id.as_deref()),
        );
        return Err(error);
    }
    if let Err(error) = ledger::finish(&state, &input.run_id, &route, &result) {
        let _ = ledger::phase(
            &state,
            &input.run_id,
            "unknown",
            result.as_ref().ok().and_then(|r| r.job_id.as_deref()),
        );
        if let Some(entry) = runs().lock().await.get_mut(&input.run_id) {
            entry.finished = true;
        }
        return Err(error);
    }
    let mut entries = runs().lock().await;
    let entry = entries
        .get_mut(&input.run_id)
        .expect("registered media run");
    entry.finished = true;
    match result {
        Ok(result) => {
            entry.result = Some(result.clone());
            Ok(GenerateOutput {
                run_id: input.run_id,
                result: Some(result),
                error: None,
            })
        }
        Err(error) => Ok(GenerateOutput {
            run_id: input.run_id,
            result: None,
            error: Some(error),
        }),
    }
}
