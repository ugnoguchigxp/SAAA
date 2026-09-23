use super::*;
pub(super) fn load_manifest(run_dir: &Path) -> Result<EndpointManifest, ContextStillRecallError> {
    if !run_dir.is_absolute() {
        return Err(ContextStillRecallError::Configuration);
    }
    validate_owner_only_directory(run_dir)?;
    let path = run_dir.join(ENDPOINT_MANIFEST_FILE);
    validate_owner_only_file(&path)?;
    let content = read_file_limited(&path, MAX_MANIFEST_BYTES)?;
    let manifest: EndpointManifest =
        serde_json::from_slice(&content).map_err(|_| ContextStillRecallError::Configuration)?;
    validate_manifest(run_dir, &manifest)?;
    Ok(manifest)
}
pub(super) fn validate_manifest(
    run_dir: &Path,
    manifest: &EndpointManifest,
) -> Result<(), ContextStillRecallError> {
    if manifest.server != "context-still"
        || manifest.transport != "streamable-http"
        || manifest.protocol_version != MCP_PROTOCOL_VERSION
        || manifest.auth != "bearer-token-file"
        || manifest.tool_profile != "typed-memory"
        || manifest.contract_version != MEMORY_RECALL_CONTRACT_VERSION
        || !valid_started_at(&manifest.started_at)
    {
        return Err(ContextStillRecallError::Configuration);
    }
    let url = Url::parse(&manifest.url).map_err(|_| ContextStillRecallError::Configuration)?;
    let loopback = match url.host() {
        Some(Host::Ipv4(address)) => address.is_loopback(),
        Some(Host::Ipv6(address)) => address.is_loopback(),
        _ => false,
    };
    if url.scheme() != "http"
        || !loopback
        || url.path() != "/mcp"
        || url.query().is_some()
        || url.fragment().is_some()
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return Err(ContextStillRecallError::Configuration);
    }
    validate_token_path(run_dir, &manifest.auth_token_path)
}
pub(super) fn valid_started_at(value: &str) -> bool {
    value
        .strip_prefix("unix-ms:")
        .filter(|millis| !millis.is_empty() && millis.bytes().all(|byte| byte.is_ascii_digit()))
        .and_then(|millis| millis.parse::<u64>().ok())
        .is_some()
}
pub(super) fn validate_token_path(
    run_dir: &Path,
    token_path: &Path,
) -> Result<(), ContextStillRecallError> {
    if !token_path.is_absolute() {
        return Err(ContextStillRecallError::Configuration);
    }
    validate_owner_only_file(token_path)?;
    let canonical_run =
        fs::canonicalize(run_dir).map_err(|_| ContextStillRecallError::Configuration)?;
    let token_parent = token_path
        .parent()
        .ok_or(ContextStillRecallError::Configuration)?;
    let canonical_parent =
        fs::canonicalize(token_parent).map_err(|_| ContextStillRecallError::Configuration)?;
    if canonical_parent != canonical_run {
        return Err(ContextStillRecallError::Configuration);
    }
    Ok(())
}
pub(super) fn read_token(
    run_dir: &Path,
    path: &Path,
) -> Result<Zeroizing<String>, ContextStillRecallError> {
    validate_token_path(run_dir, path)?;
    let content = Zeroizing::new(read_file_limited(path, MAX_TOKEN_BYTES)?);
    let token_bytes = match content.as_slice() {
        bytes if bytes.len() == 64 => bytes,
        bytes if bytes.len() == 65 && bytes.last() == Some(&b'\n') => &bytes[..64],
        bytes if bytes.len() == 66 && bytes.ends_with(b"\r\n") => &bytes[..64],
        _ => return Err(ContextStillRecallError::Authentication),
    };
    let token =
        std::str::from_utf8(token_bytes).map_err(|_| ContextStillRecallError::Authentication)?;
    if token.len() != 64 || !token.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(ContextStillRecallError::Authentication);
    }
    Ok(Zeroizing::new(token.to_string()))
}
pub(super) fn read_file_limited(
    path: &Path,
    limit: u64,
) -> Result<Vec<u8>, ContextStillRecallError> {
    let file = fs::File::open(path).map_err(|_| ContextStillRecallError::Configuration)?;
    let metadata = file
        .metadata()
        .map_err(|_| ContextStillRecallError::Configuration)?;
    if metadata.len() == 0 || metadata.len() > limit {
        return Err(ContextStillRecallError::Configuration);
    }
    let mut content = Vec::with_capacity(usize::try_from(metadata.len()).unwrap_or(0));
    file.take(limit.saturating_add(1))
        .read_to_end(&mut content)
        .map_err(|_| ContextStillRecallError::Configuration)?;
    if content.is_empty() || content.len() > limit as usize {
        return Err(ContextStillRecallError::Configuration);
    }
    Ok(content)
}
pub(super) fn validate_owner_only_directory(path: &Path) -> Result<(), ContextStillRecallError> {
    let metadata =
        fs::symlink_metadata(path).map_err(|_| ContextStillRecallError::Configuration)?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(ContextStillRecallError::Configuration);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        // SAFETY: geteuid is a process-local libc query with no pointer arguments.
        if metadata.uid() != unsafe { libc::geteuid() }
            || metadata.permissions().mode() & 0o077 != 0
        {
            return Err(ContextStillRecallError::Configuration);
        }
    }
    Ok(())
}
pub(super) fn validate_owner_only_file(path: &Path) -> Result<(), ContextStillRecallError> {
    let metadata =
        fs::symlink_metadata(path).map_err(|_| ContextStillRecallError::Configuration)?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(ContextStillRecallError::Configuration);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        // SAFETY: geteuid is a process-local libc query with no pointer arguments.
        if metadata.uid() != unsafe { libc::geteuid() }
            || metadata.permissions().mode() & 0o177 != 0
        {
            return Err(ContextStillRecallError::Configuration);
        }
    }
    Ok(())
}
pub(super) fn valid_session_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 256
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~'))
}
