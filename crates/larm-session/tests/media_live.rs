//! Opt-in end-to-end verification. Each test submits exactly one generation request.
//! SAAA_LARM_MEDIA_SUBMISSION_SECONDS (default 900) and SAAA_LARM_MEDIA_JOB_SECONDS
//! (default 1800) include cold model loading and worker shutdown. Never retry a POST.
use saaa_larm_session::media::{MediaClient, MediaError, MediaKind};
use std::{
    sync::Mutex,
    time::{Duration, Instant},
};
use tokio::sync::watch;

fn limit(name: &str, default: Duration) -> Duration {
    match std::env::var(name) {
        Ok(value) => Duration::from_secs(
            value
                .parse::<u64>()
                .ok()
                .filter(|v| (1..=86400).contains(v))
                .expect("timeout must be 1..86400 seconds"),
        ),
        Err(std::env::VarError::NotPresent) => default,
        Err(_) => panic!("invalid timeout environment variable"),
    }
}

fn checked<T>(
    result: Result<T, MediaError>,
    stage: &str,
    client: Option<&MediaClient>,
    job: Option<&str>,
    token: &str,
) -> T {
    result.unwrap_or_else(|error| {
        let report = serde_json::json!({
            "occurredAt": chrono::Utc::now().to_rfc3339(),
            "stage": stage,
            "error": error,
            "jobId": error.job_id.as_deref().or(job),
            "lastHttpResponse": client.and_then(MediaClient::last_http_response),
            "postRetried": false,
        })
        .to_string();
        let escaped = serde_json::to_string(token).unwrap();
        let report = report
            .replace(token, "[REDACTED]")
            .replace(&escaped[1..escaped.len() - 1], "[REDACTED]");
        panic!("media_live failure: {report}");
    })
}

async fn verify(kind: MediaKind) {
    let base = std::env::var("SAAA_LARM_CONTROL_URL").expect("SAAA_LARM_CONTROL_URL");
    let token = std::env::var("LARM_API_TOKEN").expect("LARM_API_TOKEN");
    assert!(!token.is_empty(), "LARM_API_TOKEN must not be empty");
    let mut client = checked(
        MediaClient::discover(&base, token.clone(), kind).await,
        "profile discovery",
        None,
        None,
        &token,
    );
    client.limits.submission = limit(
        "SAAA_LARM_MEDIA_SUBMISSION_SECONDS",
        client.limits.submission,
    );
    client.limits.job = limit("SAAA_LARM_MEDIA_JOB_SECONDS", client.limits.job);
    assert!(
        client.service().starts_on_request(),
        "media profile must advertise minWarmInstances=0"
    );
    println!(
        "at={} kind={kind:?} profile={} revision={} service={} model={} demand_start={} submission_seconds={} job_seconds={}",
        chrono::Utc::now().to_rfc3339(),
        client.profile.id,
        client.profile.revision,
        client.service().name,
        client.service().model,
        client.service().starts_on_request(),
        client.limits.submission.as_secs(),
        client.limits.job.as_secs(),
    );
    let prompt = match kind {
        MediaKind::Image => "A simple blue circle on a white background, minimal flat illustration",
        MediaKind::Music => "Gentle acoustic piano music, calm and simple melody",
    };
    let (_cancel, receiver) = watch::channel(false);
    let start = Instant::now();
    let last_progress = Mutex::new(None::<(String, Option<String>, Option<f64>)>);
    let result = client
        .generate(prompt, receiver, &|progress| {
            let next = (progress.phase, progress.job_id, progress.progress);
            let mut last = last_progress.lock().unwrap();
            if last.as_ref() != Some(&next) {
                println!(
                    "at={} elapsed={} phase={} job={:?} progress={:?}",
                    chrono::Utc::now().to_rfc3339(),
                    start.elapsed().as_secs(),
                    next.0,
                    next.1,
                    next.2
                );
                *last = Some(next);
            }
        })
        .await;
    let job = last_progress
        .lock()
        .unwrap()
        .as_ref()
        .and_then(|v| v.1.clone());
    let result = checked(result, "generation", Some(&client), job.as_deref(), &token);
    assert_eq!(result.kind, kind);
    assert!(!result.artifacts.is_empty());
    for artifact in &result.artifacts {
        // artifact_bytes also fetches and validates music's metadataUrl before its audioUrl.
        let bytes = checked(
            client.artifact_bytes(artifact).await,
            "artifact retrieval",
            Some(&client),
            result.job_id.as_deref(),
            &token,
        );
        assert!(!bytes.is_empty());
        println!(
            "at={} artifact={} mime={} bytes={} job={:?} elapsed={}",
            chrono::Utc::now().to_rfc3339(),
            artifact.id,
            artifact.mime_type,
            bytes.len(),
            result.job_id,
            start.elapsed().as_secs()
        );
    }
}

#[tokio::test]
#[ignore = "submits one real image generation via discovered public LARM API"]
async fn live_image_generation_and_artifact() {
    verify(MediaKind::Image).await;
}

#[tokio::test]
#[ignore = "submits one real 180-second music job and retrieves its artifact"]
async fn live_music_generation_and_artifact() {
    verify(MediaKind::Music).await;
}
