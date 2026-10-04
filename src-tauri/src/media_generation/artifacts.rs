//! Locally persisted artifacts, with recovery using only stored remote identity.
use super::*;

#[tauri::command]
pub(crate) async fn read_generated_media(
    state: tauri::State<'_, AppState>,
    run_id: String,
    artifact_index: usize,
) -> Result<tauri::ipc::Response, String> {
    validate_id(&run_id)?;
    if let Some(bytes) = ledger::cached(&state, &run_id, artifact_index)? {
        return Ok(tauri::ipc::Response::new(bytes));
    }
    if let Some(stored) = ledger::get(&state, &run_id)? {
        if stored["route"]["adapterKind"] == "replicate-media" {
            return Err(
                "保存済み成果物がありません。生成し直さず、進行状況の照会で取得してください。"
                    .into(),
            );
        }
    }
    if !runs().lock().await.contains_key(&run_id) {
        if let Some(stored) = ledger::get(&state, &run_id)? {
            if stored["status"] != "accepted" {
                return Err("生成の完了は確認できていません".into());
            }
            let route: ResolvedRoute = serde_json::from_value(stored["route"].clone())
                .map_err(|_| "生成設定の記録が不正です")?;
            let kind: MediaKind = serde_json::from_value(stored["kind"].clone())
                .map_err(|_| "生成の種類が不正です")?;
            let result = recovery_result(&stored["result"])?;
            let client = recovery::larm_client(&state, &route, kind)
                .await
                .map_err(|e| format!("成果物の接続先を確認できません（{}）", e.code))?;
            let (cancel, _) = watch::channel(false);
            runs().lock().await.insert(
                run_id.clone(),
                Entry {
                    cancel,
                    client: Some(client),
                    result: Some(result),
                    finished: true,
                    created: std::time::Instant::now(),
                },
            );
        }
    }
    let (client, artifact): (Arc<MediaClient>, MediaArtifact) = {
        let entries = runs().lock().await;
        let entry = entries
            .get(&run_id)
            .ok_or("成果物の参照期限が切れました。")?;
        let artifact = entry
            .result
            .as_ref()
            .and_then(|result| result.artifacts.get(artifact_index))
            .ok_or("成果物が見つかりません。")?
            .clone();
        (
            entry
                .client
                .as_ref()
                .ok_or("生成サービスを確認できません。")?
                .clone(),
            artifact,
        )
    };
    static DOWNLOADS: OnceLock<tokio::sync::Semaphore> = OnceLock::new();
    let _permit = DOWNLOADS
        .get_or_init(|| tokio::sync::Semaphore::new(2))
        .acquire()
        .await
        .map_err(|_| "成果物の取得を開始できません。")?;
    let bytes = client.artifact_bytes(&artifact).await.map_err(|error| {
        format!(
            "成果物を取得できませんでした（{}）。生成し直さず、取得を再試行できます。",
            error.code
        )
    })?;
    ledger::cache(&state, &run_id, artifact_index, &bytes)?;
    Ok(tauri::ipc::Response::new(bytes))
}

fn recovery_result(value: &Value) -> Result<MediaResult, String> {
    Ok(MediaResult {
        kind: serde_json::from_value(value["kind"].clone()).map_err(|_| "生成の種類が不正です")?,
        model: value["model"]
            .as_str()
            .ok_or("モデルの記録がありません")?
            .into(),
        job_id: value["jobId"].as_str().map(str::to_string),
        artifacts: serde_json::from_value(value["artifacts"].clone())
            .map_err(|_| "成果物の記録が不正です")?,
    })
}
