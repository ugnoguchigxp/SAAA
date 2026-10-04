//! Replicate Predictions. A submitted POST is never replayed.
//! https://replicate.com/docs/reference/http/
use super::*;
use crate::providers::service_registry::ResolvedRoute;
use saaa_larm_session::media::FailureKind;
use serde_json::{json, Value};

fn failure(kind: FailureKind, code: &str, job: Option<&str>, uncertain: bool) -> MediaError {
    MediaError {
        kind,
        code: code.into(),
        retryable: false,
        may_have_generated: uncertain,
        job_id: job.map(str::to_string),
    }
}

pub(crate) fn model_parts(model: &str) -> Result<(&str, Option<&str>), String> {
    let (name, version) = model
        .split_once(':')
        .map_or((model, None), |(name, version)| (name, Some(version)));
    let parts = name.split('/').collect::<Vec<_>>();
    if parts.len() != 2
        || parts.iter().any(|p| {
            p.is_empty()
                || p.len() > 80
                || !p
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        })
        || version.is_some_and(|v| v.len() != 64 || !v.bytes().all(|b| b.is_ascii_hexdigit()))
    {
        return Err(
            "モデルはowner/name、版を指定する場合はowner/name:64桁の版IDにしてください".into(),
        );
    }
    Ok((name, version))
}

async fn send(
    state: &AppState,
    route: &ResolvedRoute,
    method: reqwest::Method,
    operation: &str,
    body: Option<&Value>,
    budget: u64,
) -> Result<Value, MediaError> {
    state
        .sqlite_readers
        .read(|db| crate::providers::service_registry::validate_active(db, route))
        .map_err(|_| failure(FailureKind::Cancelled, "route_revoked", None, false))?;
    let secret = route
        .credential_ref
        .as_ref()
        .map(|r| {
            crate::credentials::load_named_secret(&r.service, &r.account)
                .and_then(|value| value.ok_or("API key missing".into()))
        })
        .transpose()
        .map_err(|_| {
            failure(
                FailureKind::GenerationFailed,
                "credential_missing",
                None,
                false,
            )
        })?;
    let url = saaa_larm_session::http_api::operation_url(&route.endpoint, operation)
        .map_err(|_| failure(FailureKind::Protocol, "invalid_endpoint", None, false))?;
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(std::time::Duration::from_secs(5))
        .timeout(std::time::Duration::from_millis(budget))
        .build()
        .map_err(|_| failure(FailureKind::Protocol, "client_failed", None, false))?;
    let mut request = client.request(method.clone(), url);
    if let Some(secret) = &secret {
        request = request.bearer_auth(secret.as_str());
    }
    if let Some(body) = body {
        request = request.json(body);
    }
    let mut response = request.send().await.map_err(|_| {
        failure(
            FailureKind::OutcomeUnknown,
            "request_outcome_unknown",
            None,
            method == reqwest::Method::POST,
        )
    })?;
    if !response.status().is_success() {
        if method == reqwest::Method::POST && response.status().is_server_error() {
            return Err(failure(
                FailureKind::OutcomeUnknown,
                "request_outcome_unknown",
                None,
                true,
            ));
        }
        return Err(failure(
            FailureKind::GenerationFailed,
            if response.status().as_u16() == 401 || response.status().as_u16() == 403 {
                "authentication_failed"
            } else {
                "request_rejected"
            },
            None,
            false,
        ));
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|_| {
        failure(
            FailureKind::OutcomeUnknown,
            "response_interrupted",
            None,
            true,
        )
    })? {
        if bytes.len().saturating_add(chunk.len()) > 256 * 1024 {
            return Err(failure(
                FailureKind::OutcomeUnknown,
                "response_too_large",
                None,
                true,
            ));
        }
        bytes.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&bytes)
        .map_err(|_| failure(FailureKind::OutcomeUnknown, "response_invalid", None, true))
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn generate(
    state: &AppState,
    run: &str,
    kind: MediaKind,
    prompt: &str,
    route: &ResolvedRoute,
    mut cancel: watch::Receiver<bool>,
    progress: &tauri::ipc::Channel<MediaProgress>,
    resume: Option<&str>,
) -> Result<MediaResult, MediaError> {
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_millis(route.timeout_ms);
    let budget = |deadline: tokio::time::Instant| {
        route
            .attempt_timeout_ms
            .unwrap_or(route.timeout_ms)
            .min(
                deadline
                    .saturating_duration_since(tokio::time::Instant::now())
                    .as_millis() as u64,
            )
            .max(1)
    };
    if *cancel.borrow() {
        return Err(failure(
            FailureKind::Cancelled,
            "cancelled_before_submission",
            resume,
            resume.is_some(),
        ));
    }
    let mut job = if let Some(id) = resume {
        send(
            state,
            route,
            reqwest::Method::GET,
            &format!("predictions/{id}"),
            None,
            budget(deadline),
        )
        .await
        .map_err(|mut error| {
            error.job_id = Some(id.into());
            error.may_have_generated = true;
            error
        })?
    } else {
        let (model, version) = model_parts(&route.model)
            .map_err(|_| failure(FailureKind::Protocol, "invalid_model", None, false))?;
        let mut parameters = route
            .detail
            .as_deref()
            .filter(|v| !v.trim().is_empty())
            .map(serde_json::from_str::<Value>)
            .transpose()
            .map_err(|_| {
                failure(
                    FailureKind::Protocol,
                    "invalid_input_parameters",
                    None,
                    false,
                )
            })?
            .unwrap_or(json!({}));
        if !parameters.is_object() {
            return Err(failure(
                FailureKind::Protocol,
                "invalid_input_parameters",
                None,
                false,
            ));
        }
        parameters["prompt"] = json!(prompt);
        let mut body = json!({"input":parameters});
        let operation = if let Some(version) = version {
            body["version"] = json!(version);
            "predictions".to_string()
        } else {
            format!("models/{model}/predictions")
        };
        ledger::phase(state, run, "submitting", None).map_err(|_| {
            failure(
                FailureKind::Protocol,
                "submission_record_failed",
                None,
                false,
            )
        })?;
        // Even if cancellation arrives while waiting for the POST, retain its
        // reply/job ID first. Abandoning that response would lose remote identity.
        send(
            state,
            route,
            reqwest::Method::POST,
            &operation,
            Some(&body),
            budget(deadline),
        )
        .await?
    };
    let id = job["id"]
        .as_str()
        .filter(|id| {
            !id.is_empty()
                && id.len() <= 128
                && id
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        })
        .ok_or_else(|| failure(FailureKind::OutcomeUnknown, "job_id_missing", None, true))?
        .to_string();
    if resume.is_some_and(|stored| stored != id) {
        return Err(failure(
            FailureKind::OutcomeUnknown,
            "job_identity_mismatch",
            resume,
            true,
        ));
    }
    ledger::phase(state, run, "running", Some(&id)).map_err(|_| {
        failure(
            FailureKind::OutcomeUnknown,
            "job_record_failed",
            Some(&id),
            true,
        )
    })?;
    loop {
        if *cancel.borrow() {
            ledger::phase(state, run, "cancel_requested", Some(&id)).map_err(|_| {
                failure(
                    FailureKind::OutcomeUnknown,
                    "cancel_record_failed",
                    Some(&id),
                    true,
                )
            })?;
            let response = send(
                state,
                route,
                reqwest::Method::POST,
                &format!("predictions/{id}/cancel"),
                None,
                budget(deadline),
            )
            .await;
            if response.as_ref().is_ok_and(|v| v["status"] == "canceled") {
                return Err(failure(
                    FailureKind::Cancelled,
                    "remote_cancel_confirmed",
                    Some(&id),
                    false,
                ));
            }
            return Err(failure(
                FailureKind::OutcomeUnknown,
                "remote_cancel_unconfirmed",
                Some(&id),
                true,
            ));
        }
        let status = job["status"].as_str().unwrap_or_default();
        let _ = progress.send(MediaProgress {
            phase: status.into(),
            job_id: Some(id.clone()),
            progress: None,
        });
        match status {
            "succeeded" => {
                return artifacts(state, run, kind, route, &job, &id, deadline, &mut cancel).await
            }
            "failed" => {
                return Err(failure(
                    FailureKind::GenerationFailed,
                    "remote_generation_failed",
                    Some(&id),
                    false,
                ))
            }
            "canceled" => {
                return Err(failure(
                    FailureKind::Cancelled,
                    "remote_cancel_confirmed",
                    Some(&id),
                    false,
                ))
            }
            "starting" | "processing" => {}
            _ => {
                return Err(failure(
                    FailureKind::OutcomeUnknown,
                    "invalid_job_status",
                    Some(&id),
                    true,
                ))
            }
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(failure(
                FailureKind::OutcomeUnknown,
                "generation_deadline_exceeded",
                Some(&id),
                true,
            ));
        }
        tokio::select! { _=cancel.changed()=>{}, _=tokio::time::sleep(std::time::Duration::from_secs(1))=>{} }
        if *cancel.borrow() {
            continue;
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(failure(
                FailureKind::OutcomeUnknown,
                "generation_deadline_exceeded",
                Some(&id),
                true,
            ));
        }
        job = send(
            state,
            route,
            reqwest::Method::GET,
            &format!("predictions/{id}"),
            None,
            budget(deadline),
        )
        .await
        .map_err(|mut error| {
            error.job_id = Some(id.clone());
            error.may_have_generated = true;
            error
        })?;
    }
}

#[allow(clippy::too_many_arguments)]
async fn artifacts(
    state: &AppState,
    run: &str,
    kind: MediaKind,
    route: &ResolvedRoute,
    job: &Value,
    id: &str,
    deadline: tokio::time::Instant,
    cancel: &mut watch::Receiver<bool>,
) -> Result<MediaResult, MediaError> {
    let urls = match &job["output"] {
        Value::String(url) => vec![url.as_str()],
        Value::Array(items) if !items.is_empty() && items.len() <= 8 => items
            .iter()
            .map(|v| {
                v.as_str()
                    .ok_or_else(|| failure(FailureKind::Protocol, "invalid_output", Some(id), true))
            })
            .collect::<Result<Vec<_>, _>>()?,
        _ => {
            return Err(failure(
                FailureKind::Protocol,
                "invalid_output",
                Some(id),
                true,
            ))
        }
    };
    let mut artifacts = Vec::new();
    let mut total = 0usize;
    for (index, url) in urls.iter().enumerate() {
        state
            .sqlite_readers
            .read(|db| crate::providers::service_registry::validate_active(db, route))
            .map_err(|_| {
                failure(
                    FailureKind::OutcomeUnknown,
                    "route_revoked_before_artifact",
                    Some(id),
                    true,
                )
            })?;
        if *cancel.borrow() {
            return Err(failure(
                FailureKind::Cancelled,
                "artifact_wait_cancelled",
                Some(id),
                true,
            ));
        }
        let (bytes, mime) = tokio::select! {
            _=cancel_requested(cancel)=>return Err(failure(FailureKind::Cancelled,"artifact_wait_cancelled",Some(id),true)),
            output=download(route,url,deadline)=>output.map_err(|_|failure(FailureKind::ArtifactFailed,"artifact_unavailable",Some(id),true))?,
        };
        total = total.saturating_add(bytes.len());
        if total > 64 * 1024 * 1024 {
            return Err(failure(
                FailureKind::ArtifactFailed,
                "artifacts_too_large",
                Some(id),
                true,
            ));
        }
        if !(match kind {
            MediaKind::Image => mime.starts_with("image/"),
            MediaKind::Music => mime.starts_with("audio/"),
        }) {
            return Err(failure(
                FailureKind::Protocol,
                "artifact_kind_mismatch",
                Some(id),
                true,
            ));
        }
        ledger::cache(state, run, index, &bytes).map_err(|_| {
            failure(
                FailureKind::ArtifactFailed,
                "artifact_storage_failed",
                Some(id),
                true,
            )
        })?;
        artifacts.push(MediaArtifact {
            id: format!("{run}_{index}"),
            content_url: url.to_string(),
            metadata_url: None,
            mime_type: mime,
            metadata: json!({"provider":"replicate","predictionId":id}),
        });
    }
    if *cancel.borrow() || tokio::time::Instant::now() >= deadline {
        return Err(failure(
            FailureKind::OutcomeUnknown,
            "adoption_cancelled_or_expired",
            Some(id),
            true,
        ));
    }
    Ok(MediaResult {
        kind,
        model: route.model.clone(),
        job_id: Some(id.into()),
        artifacts,
    })
}

pub(super) async fn download(
    route: &ResolvedRoute,
    raw: &str,
    deadline: tokio::time::Instant,
) -> Result<(Vec<u8>, String), String> {
    let url = url::Url::parse(raw).map_err(|_| "成果物URLが不正です")?;
    let base = url::Url::parse(&route.endpoint).map_err(|_| "接続先が不正です")?;
    let domain = url.host_str().unwrap_or_default();
    let delivered = url.scheme() == "https"
        && (domain == "replicate.delivery" || domain.ends_with(".replicate.delivery"));
    let local_fixture = base.scheme() == "http"
        && base
            .host_str()
            .is_some_and(|h| h == "127.0.0.1" || h == "[::1]")
        && url.origin() == base.origin();
    if (!delivered && !local_fixture) || !url.username().is_empty() || url.password().is_some() {
        return Err("成果物の配信元が許可された接続先と異なります".into());
    }
    let remaining = deadline
        .saturating_duration_since(tokio::time::Instant::now())
        .min(std::time::Duration::from_secs(120));
    if remaining.is_zero() {
        return Err("成果物の取得期限を超えました".into());
    }
    // Output URLs are unauthenticated. Never forward the service API key.
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(remaining)
        .build()
        .map_err(|_| "成果物の接続準備に失敗しました")?;
    let mut response = client
        .get(url)
        .send()
        .await
        .map_err(|_| "成果物へ接続できません")?;
    if !response.status().is_success() {
        return Err("成果物を取得できません".into());
    }
    let mime = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split(';').next())
        .filter(|v| {
            matches!(
                *v,
                "image/png"
                    | "image/webp"
                    | "image/jpeg"
                    | "audio/mpeg"
                    | "audio/wav"
                    | "audio/flac"
                    | "audio/ogg"
            )
        })
        .ok_or("成果物の形式を確認できません")?
        .to_string();
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| "成果物の受信が中断しました")?
    {
        if bytes.len().saturating_add(chunk.len()) > 64 * 1024 * 1024 {
            return Err("成果物が大きすぎます".into());
        }
        bytes.extend_from_slice(&chunk);
    }
    if bytes.is_empty() {
        return Err("成果物が空です".into());
    }
    Ok((bytes, mime))
}

pub(super) async fn cancel_existing(
    state: &AppState,
    route: &ResolvedRoute,
    id: &str,
) -> MediaError {
    let response = send(
        state,
        route,
        reqwest::Method::POST,
        &format!("predictions/{id}/cancel"),
        None,
        route.attempt_timeout_ms.unwrap_or(10_000).min(10_000),
    )
    .await;
    if response.as_ref().is_ok_and(|v| v["status"] == "canceled") {
        failure(
            FailureKind::Cancelled,
            "remote_cancel_confirmed",
            Some(id),
            false,
        )
    } else {
        failure(
            FailureKind::OutcomeUnknown,
            "remote_cancel_unconfirmed",
            Some(id),
            true,
        )
    }
}

async fn cancel_requested(cancel: &mut watch::Receiver<bool>) {
    while !*cancel.borrow_and_update() {
        if cancel.changed().await.is_err() {
            std::future::pending::<()>().await;
        }
    }
}
