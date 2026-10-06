use super::*;

#[tokio::test]
async fn uncertain_submission_and_foreign_artifacts_cannot_trigger_a_repeat_post_or_download() {
    for mode in [
        "invalid_submission_json",
        "foreign_artifact",
        "legacy_image",
    ] {
        let (fake, client, server) = fixture(mode, MediaKind::Image).await;
        let (_cancel, receiver) = watch::channel(false);
        let error = client
            .generate("draw", receiver, &|_| {})
            .await
            .unwrap_err();
        assert_eq!(error.kind, FailureKind::Protocol);
        assert!(error.may_have_generated);
        assert!(!error.retryable);
        assert_eq!(fake.posts.lock().unwrap().len(), 1);
        assert_eq!(fake.log.lock().unwrap().len(), 2);
        server.abort();
    }
}

#[tokio::test]
async fn non_json_failure_keeps_http_evidence_and_redacts_token_without_resubmission() {
    let (fake, client, server) = fixture("plain_failure", MediaKind::Image).await;
    let (_cancel, receiver) = watch::channel(false);
    let error = client
        .generate("draw", receiver, &|_| {})
        .await
        .unwrap_err();
    assert_eq!(error.kind, FailureKind::GenerationFailed);
    let response = client.last_http_response().unwrap();
    assert_eq!(response.status, 502);
    assert_eq!(response.body, "upstream unavailable [REDACTED]");
    assert!(chrono::DateTime::parse_from_rfc3339(&response.received_at).is_ok());
    assert_eq!(fake.posts.lock().unwrap().len(), 1);
    server.abort();
}

#[tokio::test]
async fn worker_stop_failure_is_terminal_and_preserves_failed_job_response() {
    let (fake, client, server) = fixture("music_stop_failure", MediaKind::Music).await;
    let (_cancel, receiver) = watch::channel(false);
    let error = client
        .generate("music", receiver, &|_| {})
        .await
        .unwrap_err();
    assert_eq!(error.code, "model_stop_failed");
    assert_eq!(error.job_id.as_deref(), Some("job-1"));
    assert!(!error.retryable);
    let response = client.last_http_response().unwrap();
    assert_eq!(response.status, 200);
    assert!(response.body.contains("model_stop_failed"));
    assert_eq!(fake.posts.lock().unwrap().len(), 1);
    assert!(!fake
        .log
        .lock()
        .unwrap()
        .iter()
        .any(|call| call.starts_with("GET /stored/")));
    server.abort();
}

#[tokio::test]
async fn a_failed_job_poll_preserves_the_job_without_claiming_generation_failed() {
    let (fake, client, server) = fixture("poll_failure", MediaKind::Music).await;
    let (_cancel, receiver) = watch::channel(false);
    let error = client
        .generate("music", receiver, &|_| {})
        .await
        .unwrap_err();
    assert_eq!(error.kind, FailureKind::OutcomeUnknown);
    assert_eq!(error.job_id.as_deref(), Some("job-1"));
    assert!(error.may_have_generated);
    assert!(!error.retryable);
    assert_eq!(fake.posts.lock().unwrap().len(), 1);
    server.abort();
}

#[tokio::test]
async fn mismatched_metadata_cannot_download_another_jobs_audio() {
    let (fake, client, server) = fixture("metadata_mismatch", MediaKind::Music).await;
    let (_cancel, receiver) = watch::channel(false);
    let result = client.generate("music", receiver, &|_| {}).await.unwrap();
    let error = client
        .artifact_bytes(&result.artifacts[0])
        .await
        .unwrap_err();
    assert_eq!(error.kind, FailureKind::ArtifactFailed);
    assert_eq!(error.code, "artifact_mismatch");
    assert!(!fake
        .log
        .lock()
        .unwrap()
        .iter()
        .any(|call| call == "GET /stored/audio-1"));
    assert_eq!(fake.posts.lock().unwrap().len(), 1);
    server.abort();
}

#[tokio::test]
async fn artifact_content_types_are_checked_for_both_media_kinds() {
    for kind in [MediaKind::Image, MediaKind::Music] {
        let (fake, client, server) = fixture("wrong_content_type", kind).await;
        let (_cancel, receiver) = watch::channel(false);
        let result = client.generate("prompt", receiver, &|_| {}).await.unwrap();
        let error = client
            .artifact_bytes(&result.artifacts[0])
            .await
            .unwrap_err();
        assert_eq!(error.kind, FailureKind::ArtifactFailed);
        assert_eq!(error.code, "artifact_type_mismatch");
        assert_eq!(fake.posts.lock().unwrap().len(), 1);
        server.abort();
    }
}
