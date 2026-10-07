//! In-process provider. It still sits behind MediaService and SQLite.
use saaa_larm_session::media::{FailureKind, MediaArtifact, MediaError, MediaKind, MediaResult};
use saaa_media::{GenerateCall, MediaBackend};
use saaa_provider_routing::ResolvedRoute;
use serde_json::json;
use std::sync::{
    atomic::{AtomicBool, AtomicUsize, Ordering},
    Arc,
};

pub const PNG: &[u8] = &[
    137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 13, 73, 72, 68, 82, 0, 0, 0, 1, 0, 0, 0, 1, 8, 2, 0,
    0, 0, 144, 119, 83, 222, 0, 0, 0, 12, 73, 68, 65, 84, 120, 156, 99, 248, 207, 192, 0, 0, 3, 1,
    1, 0, 201, 254, 146, 239, 0, 0, 0, 0, 73, 69, 78, 68, 174, 66, 96, 130,
];

pub struct FixtureBackend {
    pub posts: Arc<AtomicUsize>,
    pub entered: Option<Arc<AtomicBool>>,
    pub hold: Option<Arc<std::sync::Mutex<Option<tokio::sync::oneshot::Receiver<()>>>>>,
}

impl FixtureBackend {
    pub fn new() -> Self {
        Self {
            posts: Arc::new(AtomicUsize::new(0)),
            entered: None,
            hold: None,
        }
    }

    pub fn counting(posts: Arc<AtomicUsize>) -> Self {
        Self {
            posts,
            entered: None,
            hold: None,
        }
    }
}

impl Default for FixtureBackend {
    fn default() -> Self {
        Self::new()
    }
}

fn result(kind: MediaKind) -> MediaResult {
    MediaResult {
        kind,
        model: "lab-fixture".into(),
        job_id: None,
        artifacts: vec![MediaArtifact {
            id: "fixture".into(),
            content_url: "http://127.0.0.1/fixture.png".into(),
            metadata_url: None,
            mime_type: "image/png".into(),
            metadata: json!({}),
        }],
    }
}

impl MediaBackend for FixtureBackend {
    fn generate(&self, call: GenerateCall) -> saaa_media::BoxFut<Result<MediaResult, MediaError>> {
        self.posts.fetch_add(1, Ordering::SeqCst);
        if let Some(entered) = &self.entered {
            entered.store(true, Ordering::SeqCst);
        }
        let kind = call.kind;
        let hold = self.hold.clone();
        Box::pin(async move {
            if let Some(hold) = hold {
                let receiver = hold.lock().ok().and_then(|mut slot| slot.take());
                if let Some(receiver) = receiver {
                    let _ = receiver.await;
                }
            }
            (call.progress)(saaa_larm_session::media::MediaProgress {
                phase: "generating".into(),
                job_id: None,
                progress: None,
            });
            Ok(result(kind))
        })
    }

    fn reconcile(
        &self,
        _call: GenerateCall,
    ) -> saaa_media::BoxFut<Result<MediaResult, MediaError>> {
        Box::pin(async {
            Err(MediaError {
                kind: FailureKind::OutcomeUnknown,
                code: "synchronous_image_has_no_job".into(),
                retryable: false,
                may_have_generated: true,
                job_id: None,
            })
        })
    }

    fn cancel_remote(
        &self,
        _route: ResolvedRoute,
        _kind: MediaKind,
        _job_id: String,
    ) -> saaa_media::BoxFut<MediaError> {
        Box::pin(async {
            MediaError {
                kind: FailureKind::Cancelled,
                code: "remote_cancel_confirmed".into(),
                retryable: false,
                may_have_generated: false,
                job_id: None,
            }
        })
    }

    fn fetch_artifact(
        &self,
        _route: ResolvedRoute,
        _kind: MediaKind,
        _artifact: MediaArtifact,
    ) -> saaa_media::BoxFut<Result<Vec<u8>, MediaError>> {
        Box::pin(async { Ok(PNG.to_vec()) })
    }
}
