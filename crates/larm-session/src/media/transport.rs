use super::*;
use futures_util::StreamExt;
impl MediaClient {
    pub(super) async fn json(
        &self,
        call: reqwest::RequestBuilder,
        accepted: &[u16],
    ) -> Result<Value, MediaError> {
        self.json_with_location(call, accepted)
            .await
            .map(|(value, _)| value)
    }
    pub(super) async fn json_with_location(
        &self,
        call: reqwest::RequestBuilder,
        accepted: &[u16],
    ) -> Result<(Value, Option<String>), MediaError> {
        self.check_request()?;
        let is_submission = call
            .try_clone()
            .and_then(|call| call.build().ok())
            .is_some_and(|request| request.method() == reqwest::Method::POST);
        let response = crate::authorize(call, &self.token)
            .map_err(|code| MediaError::new(FailureKind::Discovery, code))?
            .send()
            .await
            .map_err(|error| {
                if is_submission {
                    MediaError::uncertain(
                        if error.is_timeout() {
                            FailureKind::Timeout
                        } else {
                            FailureKind::OutcomeUnknown
                        },
                        "submission_response_unavailable",
                    )
                } else {
                    MediaError::new(FailureKind::ArtifactFailed, "media_response_unavailable")
                }
            })?;
        let status = response.status().as_u16();
        let location = response
            .headers()
            .get("location")
            .map(|value| value.to_str().map(str::to_string))
            .transpose()
            .map_err(|_| MediaError::uncertain(FailureKind::Protocol, "invalid_job_location"))?;
        let bytes = bounded(response, 1024 * 1024).await.map_err(|mut error| {
            error.may_have_generated = is_submission;
            error
        })?;
        let value: Value = serde_json::from_slice(&bytes).map_err(|_| {
            if is_submission {
                MediaError::uncertain(FailureKind::Protocol, "invalid_media_json")
            } else {
                MediaError::new(FailureKind::Protocol, "invalid_media_json")
            }
        })?;
        if !accepted.contains(&status) {
            let code = safe_code(
                value["error"]["code"]
                    .as_str()
                    .unwrap_or("media_request_failed"),
            );
            return Err(MediaError::new(failure_kind(status, &code), &code));
        }
        Ok((value, location))
    }
    pub(super) async fn download(&self, artifact: &MediaArtifact) -> Result<Vec<u8>, MediaError> {
        self.check_request()?;
        let response = crate::authorize(
            self.client
                .get(self.url(&artifact.content_url)?)
                .timeout(self.limits.artifact),
            &self.token,
        )
        .map_err(|code| MediaError::new(FailureKind::ArtifactFailed, code))?
        .send()
        .await
        .map_err(|_| MediaError::new(FailureKind::ArtifactFailed, "artifact_transport_failed"))?;
        if response.status() != reqwest::StatusCode::OK {
            return Err(MediaError::new(
                FailureKind::ArtifactFailed,
                "artifact_unavailable",
            ));
        }
        let content_type = response
            .headers()
            .get("content-type")
            .and_then(|value| value.to_str().ok())
            .unwrap_or("")
            .split(';')
            .next()
            .unwrap_or("")
            .trim();
        if content_type != artifact.mime_type
            || !matches!(
                content_type,
                "image/png" | "image/webp" | "audio/mpeg" | "audio/wav" | "audio/flac"
            )
        {
            return Err(MediaError::new(
                FailureKind::ArtifactFailed,
                "artifact_type_mismatch",
            ));
        }
        let bytes = bounded(
            response,
            if self.kind == MediaKind::Image {
                32 * 1024 * 1024
            } else {
                128 * 1024 * 1024
            },
        )
        .await?;
        if bytes.is_empty() {
            return Err(MediaError::new(
                FailureKind::ArtifactFailed,
                "artifact_empty",
            ));
        }
        Ok(bytes)
    }
}
async fn bounded(response: reqwest::Response, limit: usize) -> Result<Vec<u8>, MediaError> {
    if response
        .content_length()
        .is_some_and(|length| length > limit as u64)
    {
        return Err(MediaError::new(
            FailureKind::Protocol,
            "media_response_too_large",
        ));
    }
    let mut stream = response.bytes_stream();
    let mut bytes = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk =
            chunk.map_err(|_| MediaError::new(FailureKind::Protocol, "media_body_incomplete"))?;
        if chunk.len() > limit.saturating_sub(bytes.len()) {
            return Err(MediaError::new(
                FailureKind::Protocol,
                "media_response_too_large",
            ));
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}
pub(super) fn safe_code(code: &str) -> String {
    if code.len() <= 128
        && !code.is_empty()
        && code
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
    {
        code.into()
    } else {
        "media_request_failed".into()
    }
}
pub(super) fn failure_kind(status: u16, code: &str) -> FailureKind {
    if status == 409 || code.contains("conflict") || code.contains("busy") {
        FailureKind::Conflict
    } else if code.contains("startup")
        || code.contains("start_failed")
        || code.contains("load_failed")
    {
        FailureKind::StartupFailed
    } else {
        FailureKind::GenerationFailed
    }
}
