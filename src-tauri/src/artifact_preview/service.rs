use super::{
    catalog::{self, Lifecycle},
    contracts::{
        ArtifactPreviewDescriptor, ArtifactPreviewPolicy, PrepareArtifactPreviewInput, APP_SCOPE,
        HTML_MEDIA_TYPE, MAX_HTML_BYTES, TOKEN_TTL_MS,
    },
    tokens::{iso_from_millis, PreviewRuntime},
};

pub(crate) fn prepare(
    runtime: &PreviewRuntime,
    input: &PrepareArtifactPreviewInput,
) -> Result<ArtifactPreviewDescriptor, String> {
    validate_id(&input.artifact_id, "preview-artifact-id")?;
    validate_id(&input.revision_id, "preview-revision-id")?;
    let revision =
        catalog::lookup(&input.artifact_id, &input.revision_id).ok_or("preview-not-found")?;
    if revision.scope != APP_SCOPE {
        return Err("preview-scope-mismatch".into());
    }
    match revision.lifecycle {
        Lifecycle::Active => {}
        Lifecycle::Archived | Lifecycle::Deleted => return Err("preview-unavailable".into()),
    }
    if revision.media_type != HTML_MEDIA_TYPE {
        return Err("preview-media-type".into());
    }
    if revision.size() > MAX_HTML_BYTES {
        return Err("preview-too-large".into());
    }
    if revision.payload.contains(&0) {
        return Err("preview-invalid-payload".into());
    }
    if std::str::from_utf8(revision.payload).is_err() {
        return Err("preview-invalid-payload".into());
    }
    let digest = revision.digest();
    let (token, label, expires_at_ms) = runtime
        .issue(revision, TOKEN_TTL_MS)
        .map_err(str::to_string)?;
    Ok(ArtifactPreviewDescriptor {
        artifact_id: revision.artifact_id.into(),
        revision_id: revision.revision_id.into(),
        title: revision.title.into(),
        media_type: HTML_MEDIA_TYPE.into(),
        digest,
        preview_token: token,
        expires_at: iso_from_millis(expires_at_ms),
        webview_label: label,
        policy: ArtifactPreviewPolicy::isolated(),
    })
}

pub(crate) fn release(runtime: &PreviewRuntime, token: &str) {
    if !token.is_empty() {
        runtime.release(token);
    }
}

fn validate_id(value: &str, error: &str) -> Result<(), String> {
    if value.is_empty()
        || value.len() > 160
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_'))
    {
        return Err(error.into());
    }
    Ok(())
}

pub(crate) fn validate_revision_bytes(bytes: &[u8], expected_digest: &str) -> Result<(), String> {
    if bytes.len() > MAX_HTML_BYTES {
        return Err("preview-too-large".into());
    }
    if bytes.contains(&0) || std::str::from_utf8(bytes).is_err() {
        return Err("preview-invalid-payload".into());
    }
    if catalog::hex_sha256(bytes) != expected_digest {
        return Err("preview-digest-mismatch".into());
    }
    Ok(())
}
