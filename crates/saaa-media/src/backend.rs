//! Live LARM and Replicate dispatch. Tests replace this with a fake backend.
use saaa_larm_session::media::{FailureKind, MediaArtifact, MediaError, MediaKind, MediaResult};
use saaa_provider_routing::{AdapterKind, ResolvedRoute};
use std::sync::Arc;

use crate::{
    contracts::{BoxFut, Clock, SystemClock},
    larm::{self, LarmSession},
    ports::{CredentialSource, GenerateCall, MediaBackend, MediaStore},
    replicate::{self, ReplicateIo},
};

pub struct LiveBackend {
    store: Arc<dyn MediaStore>,
    credentials: Arc<dyn CredentialSource>,
}

impl LiveBackend {
    pub fn new(store: Arc<dyn MediaStore>, credentials: Arc<dyn CredentialSource>) -> Self {
        Self { store, credentials }
    }

    fn io(&self, run: Option<String>) -> ReplicateIo {
        let store = self.store.clone();
        let credentials = self.credentials.clone();
        let phase_store = store.clone();
        let cache_store = store.clone();
        let phase_run = run.clone();
        let cache_run = run;
        ReplicateIo {
            validate: Arc::new(move |route| {
                phase_store
                    .validate_route(route)
                    .map_err(|error| error.message)
            }),
            load_secret: Arc::new(move |service, account| {
                credentials
                    .named_secret(service, account)
                    .map_err(|error| error.message)
            }),
            phase: Arc::new(move |phase, job| {
                let Some(run) = phase_run.as_deref() else {
                    return Err("取消の通信では台帳を更新しません".into());
                };
                cache_store
                    .record_phase(&SystemClock.now_iso(), run, phase, job)
                    .map_err(|error| error.message)
            }),
            cache: Arc::new(move |index, bytes| {
                let Some(run) = cache_run.as_deref() else {
                    return Err("取消の通信では成果物を保存しません".into());
                };
                store
                    .cache(run, index, bytes)
                    .map_err(|error| error.message)
            }),
        }
    }

    async fn larm(
        &self,
        route: &ResolvedRoute,
        kind: MediaKind,
    ) -> Result<LarmSession, MediaError> {
        let token = self
            .credentials
            .larm_token()
            .map_err(|_| missing())?
            .to_string();
        let store = self.store.clone();
        let pinned = route.clone();
        LarmSession::discover(
            route,
            kind,
            token,
            Arc::new(move || store.validate_route(&pinned).map_err(|error| error.message)),
        )
        .await
    }
}

fn missing() -> MediaError {
    MediaError {
        kind: FailureKind::Discovery,
        code: "credential_missing".into(),
        retryable: false,
        may_have_generated: false,
        job_id: None,
    }
}

fn confirmed(error: &MediaError) -> bool {
    matches!(
        error.code.as_str(),
        "remote_cancel_confirmed" | "music_cancelled"
    ) && !error.may_have_generated
}

impl MediaBackend for LiveBackend {
    fn generate(&self, call: GenerateCall) -> BoxFut<Result<MediaResult, MediaError>> {
        let backend = self.clone_refs();
        Box::pin(async move { backend.execute(call, false).await })
    }

    fn reconcile(&self, call: GenerateCall) -> BoxFut<Result<MediaResult, MediaError>> {
        let backend = self.clone_refs();
        Box::pin(async move { backend.execute(call, true).await })
    }

    fn cancel_remote(
        &self,
        route: ResolvedRoute,
        kind: MediaKind,
        job_id: String,
    ) -> BoxFut<MediaError> {
        let backend = self.clone_refs();
        Box::pin(async move {
            if route.adapter_kind == AdapterKind::ReplicateMedia {
                return replicate::cancel_existing(&backend.io(None), &route, &job_id).await;
            }
            match backend.larm(&route, kind).await {
                Ok(session) => session.cancel_music(&job_id).await,
                Err(error) => error,
            }
        })
    }

    fn fetch_artifact(
        &self,
        route: ResolvedRoute,
        kind: MediaKind,
        artifact: MediaArtifact,
    ) -> BoxFut<Result<Vec<u8>, MediaError>> {
        let backend = self.clone_refs();
        Box::pin(async move {
            if route.adapter_kind == AdapterKind::ReplicateMedia {
                return Err(MediaError {
                    kind: FailureKind::ArtifactFailed,
                    code: "stored_artifact_missing".into(),
                    retryable: false,
                    may_have_generated: true,
                    job_id: None,
                });
            }
            let session = backend.larm(&route, kind).await?;
            session.artifact(&artifact, None).await
        })
    }
}

struct Refs {
    store: Arc<dyn MediaStore>,
    credentials: Arc<dyn CredentialSource>,
}

impl LiveBackend {
    fn clone_refs(&self) -> Refs {
        Refs {
            store: self.store.clone(),
            credentials: self.credentials.clone(),
        }
    }
}

impl Refs {
    fn io(&self, run: Option<String>) -> ReplicateIo {
        LiveBackend {
            store: self.store.clone(),
            credentials: self.credentials.clone(),
        }
        .io(run)
    }

    async fn larm(
        &self,
        route: &ResolvedRoute,
        kind: MediaKind,
    ) -> Result<LarmSession, MediaError> {
        LiveBackend {
            store: self.store.clone(),
            credentials: self.credentials.clone(),
        }
        .larm(route, kind)
        .await
    }

    async fn execute(
        &self,
        call: GenerateCall,
        reconcile: bool,
    ) -> Result<MediaResult, MediaError> {
        if call.route.adapter_kind == AdapterKind::ReplicateMedia {
            return replicate::generate(
                &self.io(Some(call.run_id.clone())),
                &call.run_id,
                call.kind,
                &call.prompt,
                &call.route,
                call.cancel,
                call.progress.as_ref(),
                call.resume.as_deref(),
            )
            .await;
        }
        if reconcile {
            return self.reconcile_larm(call).await;
        }
        let session = self.larm(&call.route, call.kind).await?;
        let result = session
            .generate(&call.prompt, call.cancel, call.progress.as_ref())
            .await?;
        let store = self.store.clone();
        let run = call.run_id.clone();
        larm::cache_artifacts(&session, &result, &move |index, bytes| {
            store
                .cache(&run, index, bytes)
                .map_err(|error| error.message)
        })
        .await
    }

    async fn reconcile_larm(&self, call: GenerateCall) -> Result<MediaResult, MediaError> {
        if call.kind != MediaKind::Music {
            return Err(MediaError {
                kind: FailureKind::OutcomeUnknown,
                code: "synchronous_image_has_no_job".into(),
                retryable: false,
                may_have_generated: true,
                job_id: call.resume.clone(),
            });
        }
        let Some(job) = call.resume.clone() else {
            return Err(MediaError {
                kind: FailureKind::OutcomeUnknown,
                code: "synchronous_image_has_no_job".into(),
                retryable: false,
                may_have_generated: true,
                job_id: None,
            });
        };
        let session = self.larm(&call.route, call.kind).await?;
        session
            .resume_music(&job, call.cancel, call.progress.as_ref())
            .await
    }
}

pub fn remote_cancel_confirmed(error: &MediaError) -> bool {
    confirmed(error)
}
