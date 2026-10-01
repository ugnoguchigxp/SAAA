//! Demand-start media services. Discovery is read-only; only a user's POST starts work.
use crate::{catalog, ProfileVariant};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::time::Duration;
use tokio::sync::watch;
use url::Url;
use zeroize::Zeroizing;
#[cfg(test)]
mod tests;
mod transport;

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum MediaKind {
    Image,
    Music,
}
impl MediaKind {
    fn variant(self) -> ProfileVariant {
        match self {
            Self::Image => ProfileVariant::Image,
            Self::Music => ProfileVariant::Music,
        }
    }
    fn capability(self) -> &'static str {
        match self {
            Self::Image => "media.image.generate",
            Self::Music => "media.music.generate",
        }
    }
    fn protocol(self) -> &'static str {
        match self {
            Self::Image => "larm.image-generation.v1",
            Self::Music => "larm.music-generation.v1",
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum FailureKind {
    Discovery,
    Conflict,
    StartupFailed,
    GenerationFailed,
    Timeout,
    Cancelled,
    OutcomeUnknown,
    ArtifactFailed,
    Protocol,
}
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MediaError {
    pub kind: FailureKind,
    pub code: String,
    pub retryable: bool,
    pub may_have_generated: bool,
    pub job_id: Option<String>,
}
impl MediaError {
    fn new(kind: FailureKind, code: &str) -> Self {
        Self {
            kind,
            code: code.into(),
            retryable: matches!(
                kind,
                FailureKind::Conflict | FailureKind::StartupFailed | FailureKind::GenerationFailed
            ),
            may_have_generated: false,
            job_id: None,
        }
    }
    fn uncertain(kind: FailureKind, code: &str) -> Self {
        Self {
            may_have_generated: true,
            ..Self::new(kind, code)
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MediaProgress {
    pub phase: String,
    pub job_id: Option<String>,
    pub progress: Option<f64>,
}
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MediaArtifact {
    pub id: String,
    pub content_url: String,
    pub metadata_url: Option<String>,
    pub mime_type: String,
    pub metadata: Value,
}
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MediaResult {
    pub kind: MediaKind,
    pub model: String,
    pub job_id: Option<String>,
    pub artifacts: Vec<MediaArtifact>,
}

pub struct MediaClient {
    client: reqwest::Client,
    token: Zeroizing<String>,
    base: Url,
    pub profile: catalog::CatalogProfile,
    service: catalog::CatalogService,
    kind: MediaKind,
    pub limits: MediaLimits,
}
#[derive(Clone, Copy)]
pub struct MediaLimits {
    pub submission: Duration,
    pub job: Duration,
    pub poll: Duration,
    pub artifact: Duration,
}
impl Default for MediaLimits {
    fn default() -> Self {
        Self {
            submission: Duration::from_secs(900),
            job: Duration::from_secs(1800),
            poll: Duration::from_secs(2),
            artifact: Duration::from_secs(120),
        }
    }
}
impl MediaClient {
    pub async fn discover(base: &str, token: String, kind: MediaKind) -> Result<Self, MediaError> {
        let base = Url::parse(base)
            .map_err(|_| MediaError::new(FailureKind::Discovery, "invalid_control_url"))?;
        if !crate::local_url(&base) || base.path() != "/" {
            return Err(MediaError::new(
                FailureKind::Discovery,
                "invalid_control_url",
            ));
        }
        let client = reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(5))
            .build()
            .map_err(|_| MediaError::new(FailureKind::Discovery, "client_failed"))?;
        let profile = tokio::time::timeout(
            Duration::from_secs(30),
            catalog::fetch(&client, &base, &token, kind.variant().selector()),
        )
        .await
        .map_err(|_| MediaError::new(FailureKind::Discovery, "catalog_timeout"))?
        .map_err(|code| MediaError::new(FailureKind::Discovery, code))?;
        let matches = profile
            .services
            .iter()
            .filter(|service| {
                service.capability == kind.capability() && service.protocol == kind.protocol()
            })
            .collect::<Vec<_>>();
        if matches.len() != 1 {
            return Err(MediaError::new(
                FailureKind::Discovery,
                "service_not_advertised",
            ));
        }
        let service = matches[0].clone();
        Ok(Self {
            client,
            token: Zeroizing::new(token),
            base,
            profile,
            service,
            kind,
            limits: MediaLimits::default(),
        })
    }
    pub fn service(&self) -> &catalog::CatalogService {
        &self.service
    }
    pub async fn generate(
        &self,
        prompt: &str,
        mut cancel: watch::Receiver<bool>,
        progress: &(dyn Fn(MediaProgress) + Send + Sync),
    ) -> Result<MediaResult, MediaError> {
        if prompt.trim().is_empty() || prompt.len() > 16384 {
            return Err(MediaError::new(FailureKind::Protocol, "invalid_prompt"));
        }
        if *cancel.borrow() {
            return Err(MediaError::new(
                FailureKind::Cancelled,
                "cancelled_before_submission",
            ));
        }
        let endpoint = self.url(&self.service.endpoint)?;
        let mut body = json!({"prompt":prompt,"model":self.service.model});
        if self.kind == MediaKind::Music {
            body["durationSeconds"] = json!(180);
            body["instrumental"] = json!(false);
            body["outputFormat"] = json!("mp3");
            body["quality"] = json!("balanced");
        }
        progress(MediaProgress {
            phase: "starting".into(),
            job_id: None,
            progress: None,
        });
        let post = self.json_with_location(
            self.client
                .post(endpoint)
                .timeout(self.limits.submission)
                .json(&body),
            if self.kind == MediaKind::Image {
                &[200, 201][..]
            } else {
                &[202][..]
            },
        );
        // Music submission is allowed to finish after cancellation so we can obtain and cancel the job.
        // There is deliberately no automatic POST retry, even on transport loss.
        let (value, location) = if self.kind == MediaKind::Image {
            tokio::select! { biased;
                _ = crate::cancelled(&mut cancel) => return Err(MediaError::uncertain(FailureKind::Cancelled, "image_wait_cancelled")),
                value = post => value?,
            }
        } else {
            tokio::pin!(post);
            tokio::select! { biased;
                _ = crate::cancelled(&mut cancel) => {
                    progress(MediaProgress { phase: "cancelling".into(), job_id: None, progress: None });
                    post.await?
                },
                value = &mut post => value?,
            }
        };
        if self.kind == MediaKind::Image {
            let artifacts = self.image_artifacts(&value).map_err(|mut error| {
                error.may_have_generated = true;
                error.retryable = false;
                error
            })?;
            return Ok(MediaResult {
                kind: self.kind,
                model: self.service.model.clone(),
                job_id: None,
                artifacts,
            });
        }
        let job_id = identifier(&value["jobId"]).ok().map(str::to_string);
        self.follow_music(value, cancel, progress, location.as_deref())
            .await
            .map_err(|mut error| {
                if error.kind == FailureKind::Protocol {
                    error.may_have_generated = true;
                }
                if error.job_id.is_none() {
                    error.job_id = job_id;
                }
                error
            })
    }
    fn image_artifacts(&self, value: &Value) -> Result<Vec<MediaArtifact>, MediaError> {
        // LARM's image protocol must return persisted artifacts, never a provider readiness flag.
        let entries = value["artifacts"]
            .as_array()
            .or_else(|| value["data"].as_array())
            .filter(|entries| !entries.is_empty() && entries.len() <= 8)
            .ok_or_else(|| {
                MediaError::uncertain(FailureKind::Protocol, "missing_image_artifacts")
            })?;
        entries
            .iter()
            .map(|entry| {
                let artifact = entry.get("artifact").unwrap_or(entry);
                let id = identifier(&artifact["id"])?;
                let content_url = text(&artifact["contentUrl"])?;
                self.url(content_url)?;
                let mime = text(&artifact["mimeType"])?;
                if !matches!(mime, "image/png" | "image/webp") {
                    return Err(MediaError::new(FailureKind::Protocol, "invalid_image_type"));
                }
                Ok(MediaArtifact {
                    id: id.into(),
                    content_url: content_url.into(),
                    metadata_url: None,
                    mime_type: mime.into(),
                    metadata: artifact.clone(),
                })
            })
            .collect()
    }
    async fn follow_music(
        &self,
        mut job: Value,
        mut cancel: watch::Receiver<bool>,
        progress: &(dyn Fn(MediaProgress) + Send + Sync),
        location: Option<&str>,
    ) -> Result<MediaResult, MediaError> {
        let id = identifier(&job["jobId"])?.to_string();
        // Existing job API is a sub-resource of the discovered generation endpoint.
        let job_url = if let Some(location) = location {
            self.url(location)?
        } else {
            let mut url = self.url(&self.service.endpoint)?;
            url.path_segments_mut()
                .map_err(|_| MediaError::new(FailureKind::Protocol, "invalid_job_url"))?
                .pop_if_empty()
                .push(&id);
            url
        };
        let deadline = tokio::time::Instant::now() + self.limits.job;
        loop {
            if job["jobId"] != id {
                return Err(MediaError::new(FailureKind::Protocol, "job_mismatch"));
            }
            if *cancel.borrow() {
                return Err(self.cancel_job(job_url.clone(), &id).await);
            }
            let status = text(&job["status"])?;
            progress(MediaProgress {
                phase: status.into(),
                job_id: Some(id.clone()),
                progress: job["progress"]
                    .as_f64()
                    .filter(|value| (0.0..=1.0).contains(value)),
            });
            match status {
                "completed" => {
                    let result = &job["result"];
                    let audio = text(&result["audioUrl"])?;
                    let metadata_url = text(&result["metadataUrl"])?;
                    self.url(audio)?;
                    self.url(metadata_url)?;
                    // Completion and artifact retrieval are separate; a failed GET must not cause a new POST.
                    let metadata = result.clone();
                    let mime = match text(&result["format"])? {
                        "mp3" => "audio/mpeg",
                        "wav" => "audio/wav",
                        "flac" => "audio/flac",
                        _ => {
                            return Err(MediaError::new(
                                FailureKind::Protocol,
                                "invalid_audio_type",
                            ))
                        }
                    };
                    return Ok(MediaResult {
                        kind: self.kind,
                        model: self.service.model.clone(),
                        job_id: Some(id),
                        artifacts: vec![MediaArtifact {
                            id: identifier(&result["id"])?.into(),
                            content_url: audio.into(),
                            metadata_url: Some(metadata_url.into()),
                            mime_type: mime.into(),
                            metadata,
                        }],
                    });
                }
                "failed" => {
                    let code = job["error"]["code"]
                        .as_str()
                        .unwrap_or("music_generation_failed");
                    let mut error = MediaError::new(
                        transport::failure_kind(500, code),
                        &transport::safe_code(code),
                    );
                    error.job_id = Some(id);
                    return Err(error);
                }
                "cancelled" => {
                    return Err(MediaError {
                        job_id: Some(id),
                        ..MediaError::new(FailureKind::Cancelled, "music_cancelled")
                    })
                }
                "queued" | "loading" | "generating" | "encoding" => {}
                _ => return Err(MediaError::new(FailureKind::Protocol, "invalid_job_status")),
            }
            let next = async {
                tokio::time::sleep(self.limits.poll).await;
                self.json(
                    self.client
                        .get(job_url.clone())
                        .timeout(Duration::from_secs(30)),
                    &[200],
                )
                .await
            };
            job = tokio::select! { biased;
                _ = crate::cancelled(&mut cancel) => return Err(self.cancel_job(job_url.clone(), &id).await),
                next = tokio::time::timeout_at(deadline, next) => match next {
                    Ok(Ok(job)) => job,
                    Ok(Err(mut error)) => { error.kind = FailureKind::OutcomeUnknown; error.job_id = Some(id); error.may_have_generated = true; error.retryable = false; return Err(error); },
                    Err(_) => { let mut error = self.cancel_job(job_url.clone(), &id).await; error.kind = FailureKind::Timeout; return Err(error); },
                },
            };
        }
    }
    async fn cancel_job(&self, url: Url, id: &str) -> MediaError {
        match self
            .json(
                self.client.delete(url).timeout(Duration::from_secs(15)),
                &[200],
            )
            .await
        {
            Ok(job) if job["jobId"] == id && job["status"] == "cancelled" => MediaError {
                job_id: Some(id.into()),
                ..MediaError::new(FailureKind::Cancelled, "music_cancelled")
            },
            _ => MediaError {
                job_id: Some(id.into()),
                ..MediaError::uncertain(FailureKind::OutcomeUnknown, "music_cancel_unconfirmed")
            },
        }
    }
    fn url(&self, endpoint: &str) -> Result<Url, MediaError> {
        catalog::public_url(&self.base, endpoint)
            .map_err(|code| MediaError::new(FailureKind::Protocol, code))
    }
    pub async fn artifact_metadata(&self, artifact: &MediaArtifact) -> Result<Value, MediaError> {
        let Some(endpoint) = &artifact.metadata_url else {
            return Ok(artifact.metadata.clone());
        };
        let value = self
            .json(
                self.client
                    .get(self.url(endpoint)?)
                    .timeout(Duration::from_secs(30)),
                &[200],
            )
            .await
            .map_err(|mut error| {
                error.kind = FailureKind::ArtifactFailed;
                error.retryable = false;
                error
            })?;
        if value["id"] != artifact.id {
            return Err(MediaError::new(
                FailureKind::ArtifactFailed,
                "artifact_mismatch",
            ));
        }
        Ok(value)
    }
    pub async fn artifact_bytes(&self, artifact: &MediaArtifact) -> Result<Vec<u8>, MediaError> {
        if artifact.metadata_url.is_some() {
            self.artifact_metadata(artifact).await?;
        }
        self.download(artifact).await.map_err(|mut error| {
            error.kind = FailureKind::ArtifactFailed;
            error.retryable = false;
            error
        })
    }
}
fn text(value: &Value) -> Result<&str, MediaError> {
    value
        .as_str()
        .filter(|value| !value.is_empty() && value.len() <= 4096)
        .ok_or_else(|| MediaError::new(FailureKind::Protocol, "invalid_media_contract"))
}
fn identifier(value: &Value) -> Result<&str, MediaError> {
    let value = text(value)?;
    if value.len() > 128
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
        || matches!(value, "." | "..")
    {
        return Err(MediaError::new(FailureKind::Protocol, "invalid_media_id"));
    }
    Ok(value)
}
