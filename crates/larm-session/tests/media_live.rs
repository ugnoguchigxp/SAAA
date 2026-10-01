//! Opt-in end-to-end verification. Each test submits exactly one generation request.
use saaa_larm_session::media::{MediaClient, MediaKind};
use std::time::Instant;
use tokio::sync::watch;

async fn verify(kind: MediaKind) {
    let base = std::env::var("SAAA_LARM_CONTROL_URL").expect("SAAA_LARM_CONTROL_URL");
    let token = std::env::var("LARM_API_TOKEN").expect("LARM_API_TOKEN");
    let client = MediaClient::discover(&base, token, kind)
        .await
        .expect("profile discovery");
    println!(
        "kind={kind:?} profile={} service={} model={} demand_start={}",
        client.profile.id,
        client.service().name,
        client.service().model,
        client.service().starts_on_request()
    );
    let prompt = match kind {
        MediaKind::Image => "A simple blue circle on a white background, minimal flat illustration",
        MediaKind::Music => "Gentle acoustic piano music, calm and simple melody",
    };
    let (_cancel, receiver) = watch::channel(false);
    let start = Instant::now();
    let result = client
        .generate(prompt, receiver, &|progress| {
            println!(
                "elapsed={} phase={} job={:?} progress={:?}",
                start.elapsed().as_secs(),
                progress.phase,
                progress.job_id,
                progress.progress
            );
        })
        .await
        .expect("generation must return actual artifacts");
    assert_eq!(result.kind, kind);
    assert!(!result.artifacts.is_empty());
    for artifact in &result.artifacts {
        let metadata = client
            .artifact_metadata(artifact)
            .await
            .expect("artifact metadata");
        assert_eq!(metadata["id"], artifact.id);
        let bytes = client
            .artifact_bytes(artifact)
            .await
            .expect("artifact content");
        assert!(!bytes.is_empty());
        println!(
            "artifact={} mime={} bytes={} elapsed={}",
            artifact.id,
            artifact.mime_type,
            bytes.len(),
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
