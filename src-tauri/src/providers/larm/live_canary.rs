use super::{
    client::{Cancellation, ChatMessage, CleanupResult, SharedLarmClient},
    contracts::SessionFailureKind,
    AllocationCleanup, LarmProvider, LarmRuntimeGate, CANARY_PROMPTS, CONTRACT_COMMIT,
};
use serde::Serialize;
use std::{
    env,
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    process::Stdio,
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};
use time::{format_description::well_known::Rfc3339, OffsetDateTime};
use tokio::process::Command;
use tokio::sync::Notify;

const REPORT_FORMAT: &str = "saaa-larm-readiness-v1";
const SAMPLE_INTERVAL: Duration = Duration::from_secs(5);
const RELEASE_RECOVERY_LIMIT: Duration = Duration::from_secs(10);
const PROMPT_EXACT_OK: &str = CANARY_PROMPTS[0];
const PROMPT_NUMBERS: &str = CANARY_PROMPTS[1];
const PROMPT_CANCEL: &str = CANARY_PROMPTS[3];

#[derive(Debug)]
enum CanaryError {
    Gate,
    Environment,
    Contract,
    Health,
    Ready,
    Authentication,
    Redaction,
    Metrics,
    Sampling,
    AllocationLeak,
    Allocation,
    Gateway,
    Cancel,
    Renew,
    Release,
    Ttl,
    Report,
}

impl CanaryError {
    fn failure_code(&self) -> &'static str {
        match self {
            Self::Gate => "gate-missing",
            Self::Environment => "environment-invalid",
            Self::Contract | Self::Metrics | Self::Renew => "contract-mismatch",
            Self::Health => "health-failed",
            Self::Ready => "ready-failed",
            Self::Authentication => "authentication-failed",
            Self::Redaction => "redaction-failed",
            Self::Sampling => "sampling-gap",
            Self::AllocationLeak | Self::Release => "allocation-leak",
            Self::Allocation => "allocation-failed",
            Self::Gateway => "gateway-failed",
            Self::Cancel => "cancel-failed",
            Self::Ttl => "ttl-recovery-failed",
            Self::Report => "report-schema-invalid",
        }
    }

    fn result(&self) -> &'static str {
        match self {
            Self::Gate | Self::Environment => "blocked",
            _ => "failed",
        }
    }
}

#[derive(Clone)]
struct CanaryContext {
    base_url: String,
    result_file: PathBuf,
    saaa_commit: String,
    artifact_sha256: String,
    manifest_sha256: String,
    deployment_revision: String,
}

#[derive(Default, Serialize)]
#[serde(rename_all = "camelCase")]
struct ScenarioCounts {
    normal_turns: u64,
    cancellations: u64,
    request_timeouts: u64,
    partial_interruptions: u64,
    larm_restarts: u64,
    saaa_restarts: u64,
    capacity_rejections: u64,
    ttl_recoveries: u64,
    renewals: u64,
    rollback_preflight_turns: u64,
    settings_rollback_turns: u64,
    kill_switch_rollback_turns: u64,
}

#[derive(Default, Serialize)]
#[serde(rename_all = "camelCase")]
struct ResultCounts {
    completed: u64,
    cancelled: u64,
    expected_failures: u64,
    unexpected_failures: u64,
    duplicate_terminals: u64,
    explicit_provider_fallbacks: u64,
    implicit_fallbacks: u64,
    stale_allocation_reuses: u64,
    runtime_policy_violations: u64,
    leaked_allocations: u64,
}

#[derive(Default, Serialize)]
#[serde(rename_all = "camelCase")]
struct TimingSummary {
    elapsed_ms: u64,
    sample_interval_seconds: u64,
    rss_max_sampling_gap_seconds: u64,
    metrics_max_sampling_gap_seconds: u64,
    planned_larm_restart_gap_seconds: u64,
    release_recovery_max_ms: u64,
    ttl_recovery_max_ms: u64,
}

#[derive(Default, Serialize)]
#[serde(rename_all = "camelCase")]
struct ResourceSummary {
    baseline_active_allocations: u64,
    max_active_allocations: u64,
    final_active_allocations: u64,
    rss_range_mi_b: u64,
    rss_previous30m_median_mi_b: u64,
    rss_last30m_median_mi_b: u64,
}

#[derive(Default, Serialize)]
#[serde(rename_all = "camelCase")]
struct LeaseSummary {
    effective_ttl_seconds_min: u64,
    effective_ttl_seconds_max: u64,
    renewals_attempted: u64,
    renewals_succeeded: u64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ReadinessReport {
    format: &'static str,
    saaa_commit: String,
    saaa_artifact_sha256: String,
    canary_manifest_sha256: String,
    larm_contract_commit: &'static str,
    deployment_revision: String,
    started_at: String,
    finished_at: String,
    mode: &'static str,
    scenario_counts: ScenarioCounts,
    result_counts: ResultCounts,
    timing_summary: TimingSummary,
    resource_summary: ResourceSummary,
    lease_summary: LeaseSummary,
    failure_codes: Vec<&'static str>,
    redaction_check: &'static str,
    result: &'static str,
}

impl ReadinessReport {
    fn new(context: &CanaryContext, mode: &'static str) -> Result<Self, CanaryError> {
        let now = timestamp()?;
        Ok(Self {
            format: REPORT_FORMAT,
            saaa_commit: context.saaa_commit.clone(),
            saaa_artifact_sha256: context.artifact_sha256.clone(),
            canary_manifest_sha256: context.manifest_sha256.clone(),
            larm_contract_commit: CONTRACT_COMMIT,
            deployment_revision: context.deployment_revision.clone(),
            started_at: now.clone(),
            finished_at: now,
            mode,
            scenario_counts: ScenarioCounts::default(),
            result_counts: ResultCounts::default(),
            timing_summary: TimingSummary::default(),
            resource_summary: ResourceSummary::default(),
            lease_summary: LeaseSummary::default(),
            failure_codes: Vec::new(),
            redaction_check: "passed",
            result: "passed",
        })
    }

    fn finish(&mut self, started: Instant) -> Result<(), CanaryError> {
        self.timing_summary.elapsed_ms = duration_millis_ceil(started.elapsed()).min(10_800_000);
        self.finished_at = timestamp()?;
        Ok(())
    }

    fn record_failure(&mut self, error: &CanaryError) {
        self.failure_codes = vec![error.failure_code()];
        if matches!(error, CanaryError::Redaction) {
            self.redaction_check = "failed";
        }
        self.result = error.result();
    }
}

fn duration_millis_ceil(duration: Duration) -> u64 {
    u64::try_from(duration.as_nanos().saturating_add(999_999) / 1_000_000).unwrap_or(u64::MAX)
}

fn duration_seconds_ceil(duration: Duration) -> u64 {
    u64::try_from(duration.as_nanos().saturating_add(999_999_999) / 1_000_000_000)
        .unwrap_or(u64::MAX)
}

fn timestamp() -> Result<String, CanaryError> {
    OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .map_err(|_| CanaryError::Report)
}

fn bounded_environment(name: &str, minimum: usize, maximum: usize) -> Result<String, CanaryError> {
    let value = env::var(name).map_err(|_| CanaryError::Environment)?;
    if value.len() < minimum
        || value.len() > maximum
        || value
            .bytes()
            .any(|byte| byte.is_ascii_whitespace() || byte.is_ascii_control())
    {
        return Err(CanaryError::Environment);
    }
    Ok(value)
}

fn lowercase_hex(value: &str, minimum: usize, maximum: usize) -> bool {
    (minimum..=maximum).contains(&value.len())
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn revision(value: &str) -> bool {
    (1..=64).contains(&value.len())
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

fn context(expected_filename: &str) -> Result<CanaryContext, CanaryError> {
    if env::var("SAAA_LARM_CANARY").as_deref() != Ok("1")
        || env::var("SAAA_LARM_ENABLED").as_deref() != Ok("1")
    {
        return Err(CanaryError::Gate);
    }
    let _credential = super::client::EphemeralCredential::from_environment()
        .map_err(|_| CanaryError::Authentication)?;
    let base_url = bounded_environment("SAAA_LARM_CANARY_BASE_URL", 1, 2_048)?;
    let saaa_commit = bounded_environment("SAAA_LARM_CANARY_SAAA_COMMIT", 7, 64)?;
    let deployed_commit = bounded_environment("SAAA_LARM_DEPLOYED_COMMIT", 7, 64)?;
    let deployment_revision = bounded_environment("SAAA_LARM_DEPLOYMENT_REVISION", 1, 64)?;
    let artifact_sha256 = bounded_environment("SAAA_LARM_CANARY_ARTIFACT_SHA256", 64, 64)?;
    let manifest_sha256 = bounded_environment("SAAA_LARM_CANARY_MANIFEST_SHA256", 64, 64)?;
    let metrics_scope = bounded_environment("SAAA_LARM_CANARY_METRICS_SCOPE", 13, 16)?;
    let result_file =
        PathBuf::from(env::var_os("SAAA_LARM_CANARY_RESULT_FILE").ok_or(CanaryError::Environment)?);
    if !lowercase_hex(&saaa_commit, 7, 64)
        || deployed_commit != CONTRACT_COMMIT
        || !lowercase_hex(&deployed_commit, 7, 64)
        || !revision(&deployment_revision)
        || !lowercase_hex(&artifact_sha256, 64, 64)
        || !lowercase_hex(&manifest_sha256, 64, 64)
        || !matches!(metrics_scope.as_str(), "exclusive-window" | "client-scoped")
        || !result_file.is_absolute()
        || result_file.file_name().and_then(|value| value.to_str()) != Some(expected_filename)
        || result_file.exists()
    {
        return Err(CanaryError::Environment);
    }
    let parent = result_file.parent().ok_or(CanaryError::Environment)?;
    if parent
        .canonicalize()
        .map_err(|_| CanaryError::Environment)?
        != parent
    {
        return Err(CanaryError::Environment);
    }
    validate_private_directory(parent)?;
    Ok(CanaryContext {
        base_url,
        result_file,
        saaa_commit,
        artifact_sha256,
        manifest_sha256,
        deployment_revision,
    })
}

#[cfg(unix)]
fn validate_private_directory(directory: &Path) -> Result<(), CanaryError> {
    use std::os::unix::fs::PermissionsExt;
    let metadata = directory
        .symlink_metadata()
        .map_err(|_| CanaryError::Environment)?;
    if !metadata.is_dir()
        || metadata.file_type().is_symlink()
        || metadata.permissions().mode() & 0o777 != 0o700
    {
        return Err(CanaryError::Environment);
    }
    Ok(())
}

#[cfg(not(unix))]
fn validate_private_directory(directory: &Path) -> Result<(), CanaryError> {
    let metadata = directory
        .symlink_metadata()
        .map_err(|_| CanaryError::Environment)?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(CanaryError::Environment);
    }
    Ok(())
}

#[cfg(unix)]
fn create_private(filename: &Path) -> Result<std::fs::File, CanaryError> {
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
    let file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(filename)
        .map_err(|_| CanaryError::Report)?;
    file.set_permissions(fs::Permissions::from_mode(0o600))
        .map_err(|_| CanaryError::Report)?;
    Ok(file)
}

#[cfg(not(unix))]
fn create_private(filename: &Path) -> Result<std::fs::File, CanaryError> {
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(filename)
        .map_err(|_| CanaryError::Report)
}

fn publish(context: &CanaryContext, report: &ReadinessReport) -> Result<(), CanaryError> {
    let parent = context.result_file.parent().ok_or(CanaryError::Report)?;
    let temporary = parent.join(format!(
        ".{}.{}.tmp",
        context
            .result_file
            .file_name()
            .and_then(|value| value.to_str())
            .ok_or(CanaryError::Report)?,
        uuid::Uuid::new_v4().simple()
    ));
    let published = (|| {
        let mut file = create_private(&temporary)?;
        let bytes = serde_json::to_vec(report).map_err(|_| CanaryError::Report)?;
        if bytes.len().saturating_add(1) > 64 * 1_024 {
            return Err(CanaryError::Report);
        }
        file.write_all(&bytes).map_err(|_| CanaryError::Report)?;
        file.write_all(b"\n").map_err(|_| CanaryError::Report)?;
        file.sync_all().map_err(|_| CanaryError::Report)?;
        validate_private_file(&file, parent)?;
        drop(file);
        fs::hard_link(&temporary, &context.result_file).map_err(|_| CanaryError::Report)
    })();
    let removed = fs::remove_file(&temporary).map_err(|_| CanaryError::Report);
    published.and(removed)
}

#[cfg(unix)]
fn validate_private_file(file: &std::fs::File, parent: &Path) -> Result<(), CanaryError> {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    let file_metadata = file.metadata().map_err(|_| CanaryError::Report)?;
    let parent_metadata = parent.metadata().map_err(|_| CanaryError::Report)?;
    if !file_metadata.is_file()
        || file_metadata.nlink() != 1
        || file_metadata.permissions().mode() & 0o777 != 0o600
        || file_metadata.uid() != parent_metadata.uid()
    {
        return Err(CanaryError::Report);
    }
    Ok(())
}

#[cfg(not(unix))]
fn validate_private_file(file: &std::fs::File, _parent: &Path) -> Result<(), CanaryError> {
    if !file.metadata().map_err(|_| CanaryError::Report)?.is_file() {
        return Err(CanaryError::Report);
    }
    Ok(())
}

fn cancellation() -> (AtomicBool, Notify) {
    (AtomicBool::new(false), Notify::new())
}

async fn metrics(client: &SharedLarmClient, base_url: &str) -> Result<u64, CanaryError> {
    client
        .canary_active_allocations(base_url)
        .await
        .map_err(|error| match error {
            SessionFailureKind::Policy => CanaryError::Redaction,
            SessionFailureKind::Network
            | SessionFailureKind::Timeout
            | SessionFailureKind::Unavailable
            | SessionFailureKind::Upstream
            | SessionFailureKind::NotReady => CanaryError::Sampling,
            _ => CanaryError::Metrics,
        })
}

async fn wait_for_allocation_count(
    client: &SharedLarmClient,
    base_url: &str,
    expected: u64,
    deadline: Duration,
) -> Result<Duration, CanaryError> {
    let started = Instant::now();
    loop {
        if metrics(client, base_url).await? == expected {
            return Ok(started.elapsed());
        }
        if started.elapsed() >= deadline {
            return Err(CanaryError::AllocationLeak);
        }
        tokio::time::sleep(SAMPLE_INTERVAL).await;
    }
}

async fn normal_turn(
    provider: &LarmProvider<'_>,
    prompt: &'static str,
    expected: Option<&'static str>,
) -> Result<(u32, Duration), CanaryError> {
    let (flag, notify) = cancellation();
    let signal = Cancellation {
        flag: &flag,
        notify: &notify,
    };
    let lease = provider
        .allocate_ready(signal)
        .await
        .map_err(|_| CanaryError::Allocation)?;
    let ttl = lease.effective_ttl_seconds;
    let messages = [ChatMessage {
        role: "user",
        content: prompt.to_string(),
    }];
    let completion = provider
        .chat(
            &lease,
            &messages,
            Duration::from_secs(120),
            signal,
            |_, _| Ok(()),
        )
        .await;
    let release_started = Instant::now();
    if provider.release(&lease.allocation_id).await != CleanupResult::Released {
        return Err(CanaryError::Release);
    }
    let completion = completion.map_err(|_| CanaryError::Gateway)?;
    if expected.is_some_and(|expected| completion.content.trim() != expected) {
        return Err(CanaryError::Gateway);
    }
    Ok((ttl, release_started.elapsed()))
}

async fn cancelled_turn(provider: &LarmProvider<'_>) -> Result<(u32, Duration), CanaryError> {
    let (flag, notify) = cancellation();
    let signal = Cancellation {
        flag: &flag,
        notify: &notify,
    };
    let lease = provider
        .allocate_ready(signal)
        .await
        .map_err(|_| CanaryError::Allocation)?;
    let ttl = lease.effective_ttl_seconds;
    let messages = [ChatMessage {
        role: "user",
        content: PROMPT_CANCEL.to_string(),
    }];
    let result = provider
        .chat(
            &lease,
            &messages,
            Duration::from_secs(120),
            signal,
            |delta, _| {
                if !delta.is_empty() && !flag.swap(true, Ordering::SeqCst) {
                    notify.notify_waiters();
                }
                Ok(())
            },
        )
        .await;
    let release_started = Instant::now();
    if provider.release(&lease.allocation_id).await != CleanupResult::Released {
        return Err(CanaryError::Release);
    }
    if !matches!(
        result,
        Err(error) if error.kind == SessionFailureKind::Cancelled && error.output_started
    ) {
        return Err(CanaryError::Cancel);
    }
    Ok((ttl, release_started.elapsed()))
}

async fn renew_once(provider: &LarmProvider<'_>) -> Result<(u32, Duration), CanaryError> {
    let (flag, notify) = cancellation();
    let signal = Cancellation {
        flag: &flag,
        notify: &notify,
    };
    let original = provider
        .allocate_ready(signal)
        .await
        .map_err(|_| CanaryError::Allocation)?;
    let renewed = match provider.renew_for_canary(&original, signal).await {
        Ok(renewed) => renewed,
        Err(_) => {
            let _ = provider.release(&original.allocation_id).await;
            return Err(CanaryError::Renew);
        }
    };
    if !original.same_binding_as(&renewed) {
        let _ = provider.release(&renewed.allocation_id).await;
        if original.allocation_id != renewed.allocation_id {
            let _ = provider.release(&original.allocation_id).await;
        }
        return Err(CanaryError::Contract);
    }
    let ttl = renewed.effective_ttl_seconds;
    let release_started = Instant::now();
    if provider.release(&renewed.allocation_id).await != CleanupResult::Released {
        return Err(CanaryError::Release);
    }
    Ok((ttl, release_started.elapsed()))
}

async fn wait_for_ttl_recovery(
    client: &SharedLarmClient,
    base_url: &str,
    baseline: u64,
    ttl: u32,
) -> Result<Duration, CanaryError> {
    wait_for_allocation_count(
        client,
        base_url,
        baseline.saturating_add(1),
        RELEASE_RECOVERY_LIMIT,
    )
    .await
    .map_err(ttl_wait_error)?;
    wait_for_allocation_count(
        client,
        base_url,
        baseline,
        Duration::from_secs(u64::from(ttl) + 30),
    )
    .await
    .map_err(ttl_wait_error)
}

fn ttl_wait_error(error: CanaryError) -> CanaryError {
    if matches!(&error, CanaryError::AllocationLeak) {
        CanaryError::Ttl
    } else {
        error
    }
}

async fn ttl_create_response_interruption(
    provider: &LarmProvider<'_>,
    client: &SharedLarmClient,
    base_url: &str,
    baseline: u64,
) -> Result<Duration, CanaryError> {
    let (flag, notify) = cancellation();
    let failure = match provider
        .allocate_ready(Cancellation {
            flag: &flag,
            notify: &notify,
        })
        .await
    {
        Ok(_) => return Err(CanaryError::Ttl),
        Err(failure) => failure,
    };
    if failure.kind != SessionFailureKind::AllocationOutcomeUnknown
        || !matches!(failure.cleanup, AllocationCleanup::DeferredToTtl(_))
    {
        return Err(CanaryError::Ttl);
    }
    wait_for_ttl_recovery(client, base_url, baseline, 300).await
}

async fn ttl_release_interruption(
    provider: &LarmProvider<'_>,
    client: &SharedLarmClient,
    base_url: &str,
    baseline: u64,
) -> Result<(u32, Duration), CanaryError> {
    let (flag, notify) = cancellation();
    let signal = Cancellation {
        flag: &flag,
        notify: &notify,
    };
    let lease = provider
        .allocate_ready(signal)
        .await
        .map_err(|_| CanaryError::Allocation)?;
    let ttl = lease.effective_ttl_seconds;
    let messages = [ChatMessage {
        role: "user",
        content: PROMPT_EXACT_OK.to_string(),
    }];
    let completion = provider
        .chat(
            &lease,
            &messages,
            Duration::from_secs(120),
            signal,
            |_, _| Ok(()),
        )
        .await;
    if completion.is_err() {
        let _ = provider.release(&lease.allocation_id).await;
        return Err(CanaryError::Gateway);
    }
    if !matches!(
        provider.release(&lease.allocation_id).await,
        CleanupResult::DeferredToTtl(_)
    ) {
        return Err(CanaryError::Ttl);
    }
    let elapsed = wait_for_ttl_recovery(client, base_url, baseline, ttl).await?;
    Ok((ttl, elapsed))
}

async fn ttl_client_exit_helper() -> Result<(), CanaryError> {
    let context = context(".functional-rust.json")?;
    let helper_file = PathBuf::from(
        env::var_os("SAAA_LARM_CANARY_TTL_HELPER_FILE").ok_or(CanaryError::Environment)?,
    );
    if helper_file.parent() != context.result_file.parent()
        || helper_file.file_name().and_then(|value| value.to_str()) != Some(".ttl-client-exit.txt")
        || helper_file.exists()
    {
        return Err(CanaryError::Environment);
    }
    let gate = LarmRuntimeGate::initialize();
    let provider = LarmProvider::for_attempt(&gate, &context.base_url, 300, 300)
        .map_err(|_| CanaryError::Contract)?;
    let (flag, notify) = cancellation();
    let lease = provider
        .allocate_ready(Cancellation {
            flag: &flag,
            notify: &notify,
        })
        .await
        .map_err(|_| CanaryError::Allocation)?;
    let mut file = create_private(&helper_file)?;
    writeln!(file, "{}", lease.effective_ttl_seconds).map_err(|_| CanaryError::Report)?;
    file.sync_all().map_err(|_| CanaryError::Report)?;
    validate_private_file(&file, helper_file.parent().ok_or(CanaryError::Environment)?)?;
    drop(lease);
    Ok(())
}

async fn ttl_client_exit_before_gateway(
    context: &CanaryContext,
    client: &SharedLarmClient,
    base_url: &str,
    baseline: u64,
) -> Result<(u32, Duration), CanaryError> {
    let executable = env::current_exe().map_err(|_| CanaryError::Environment)?;
    let helper_file = context
        .result_file
        .parent()
        .ok_or(CanaryError::Environment)?
        .join(".ttl-client-exit.txt");
    if helper_file.exists() {
        return Err(CanaryError::Environment);
    }
    let mut command = Command::new(executable);
    command
        .env_clear()
        .args([
            "providers::larm::live_canary::live_functional",
            "--ignored",
            "--exact",
            "--test-threads=1",
        ])
        .env("SAAA_LARM_CANARY_HELPER", "ttl-client-exit")
        .env("SAAA_LARM_CANARY_TTL_HELPER_FILE", &helper_file)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    for key in [
        "SAAA_LARM_CANARY",
        "SAAA_LARM_ENABLED",
        "SAAA_LARM_CANARY_BASE_URL",
        "SAAA_LARM_DEPLOYED_COMMIT",
        "SAAA_LARM_DEPLOYMENT_REVISION",
        "SAAA_LARM_CANARY_RESULT_FILE",
        "SAAA_LARM_CANARY_ARTIFACT_SHA256",
        "SAAA_LARM_CANARY_MANIFEST_SHA256",
        "SAAA_LARM_CANARY_SAAA_COMMIT",
        "SAAA_LARM_CANARY_METRICS_SCOPE",
        "LARM_API_TOKEN",
    ] {
        let value = env::var_os(key).ok_or(CanaryError::Environment)?;
        command.env(key, value);
    }
    let mut child = command.spawn().map_err(|_| CanaryError::Environment)?;
    let status = match tokio::time::timeout(Duration::from_secs(310), child.wait()).await {
        Ok(Ok(status)) => status,
        Ok(Err(_)) | Err(_) => {
            let _ = child.kill().await;
            let _ = fs::remove_file(&helper_file);
            return Err(CanaryError::Allocation);
        }
    };
    if !status.success() {
        let _ = fs::remove_file(&helper_file);
        return Err(CanaryError::Allocation);
    }
    let ttl_result = (|| {
        #[cfg(unix)]
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        let metadata = helper_file
            .symlink_metadata()
            .map_err(|_| CanaryError::Report)?;
        let parent_metadata = context
            .result_file
            .parent()
            .ok_or(CanaryError::Report)?
            .metadata()
            .map_err(|_| CanaryError::Report)?;
        if !metadata.is_file() || metadata.file_type().is_symlink() || metadata.len() > 16 {
            return Err(CanaryError::Report);
        }
        #[cfg(unix)]
        if metadata.nlink() != 1
            || metadata.permissions().mode() & 0o777 != 0o600
            || metadata.uid() != parent_metadata.uid()
        {
            return Err(CanaryError::Report);
        }
        let value = fs::read_to_string(&helper_file).map_err(|_| CanaryError::Report)?;
        let ttl = value
            .trim()
            .parse::<u32>()
            .map_err(|_| CanaryError::Report)?;
        if !(60..=300).contains(&ttl) {
            return Err(CanaryError::Contract);
        }
        Ok(ttl)
    })();
    let _ = fs::remove_file(&helper_file);
    let ttl = ttl_result?;
    let elapsed = wait_for_ttl_recovery(client, base_url, baseline, ttl).await?;
    Ok((ttl, elapsed))
}

fn update_ttl(summary: &mut LeaseSummary, ttl: u32) {
    let ttl = u64::from(ttl);
    summary.effective_ttl_seconds_min = if summary.effective_ttl_seconds_min == 0 {
        ttl
    } else {
        summary.effective_ttl_seconds_min.min(ttl)
    };
    summary.effective_ttl_seconds_max = summary.effective_ttl_seconds_max.max(ttl);
}

#[tokio::test]
#[ignore = "operator-only live LARM preflight"]
async fn live_preflight() -> Result<(), CanaryError> {
    let context = context(".preflight-rust.json")?;
    let started = Instant::now();
    let mut report = ReadinessReport::new(&context, "preflight")?;
    let outcome: Result<(), CanaryError> = async {
        let shared = SharedLarmClient::build().map_err(|_| CanaryError::Health)?;
        let baseline = metrics(&shared, &context.base_url).await?;
        if baseline != 0 {
            return Err(CanaryError::AllocationLeak);
        }
        shared
            .canary_health(&context.base_url)
            .await
            .map_err(|_| CanaryError::Health)?;
        shared
            .canary_ready(&context.base_url)
            .await
            .map_err(|_| CanaryError::Ready)?;
        shared
            .canary_authentication_boundary(&context.base_url)
            .await
            .map_err(|_| CanaryError::Authentication)?;
        let final_count = metrics(&shared, &context.base_url).await?;
        if final_count != baseline {
            return Err(CanaryError::AllocationLeak);
        }
        report.resource_summary.baseline_active_allocations = baseline;
        report.resource_summary.max_active_allocations = baseline;
        report.resource_summary.final_active_allocations = final_count;
        Ok(())
    }
    .await;
    if let Err(error) = outcome {
        report.record_failure(&error);
    }
    report.finish(started)?;
    publish(&context, &report)
}

#[tokio::test]
#[ignore = "operator-only live LARM functional canary"]
async fn live_functional() -> Result<(), CanaryError> {
    if env::var("SAAA_LARM_CANARY_HELPER").as_deref() == Ok("ttl-client-exit") {
        return ttl_client_exit_helper().await;
    }
    let context = context(".functional-rust.json")?;
    let started = Instant::now();
    let mut report = ReadinessReport::new(&context, "functional")?;
    let outcome: Result<(), CanaryError> = async {
        let shared = SharedLarmClient::build().map_err(|_| CanaryError::Health)?;
        let baseline = metrics(&shared, &context.base_url).await?;
        if baseline != 0 {
            return Err(CanaryError::AllocationLeak);
        }
        let gate = LarmRuntimeGate::initialize();
        let provider = LarmProvider::for_attempt(&gate, &context.base_url, 300, 300)
            .map_err(|_| CanaryError::Contract)?;
        let mut release_recovery = Duration::ZERO;
        for (prompt, expected) in [(PROMPT_EXACT_OK, Some("CANARY_OK")), (PROMPT_NUMBERS, None)] {
            let (ttl, released) = normal_turn(&provider, prompt, expected).await?;
            update_ttl(&mut report.lease_summary, ttl);
            let recovered = wait_for_allocation_count(
                &shared,
                &context.base_url,
                baseline,
                RELEASE_RECOVERY_LIMIT.saturating_sub(released),
            )
            .await?;
            release_recovery = release_recovery.max(released + recovered);
            report.scenario_counts.normal_turns += 1;
            report.result_counts.completed += 1;
        }
        let (ttl, released) = cancelled_turn(&provider).await?;
        update_ttl(&mut report.lease_summary, ttl);
        let recovered = wait_for_allocation_count(
            &shared,
            &context.base_url,
            baseline,
            RELEASE_RECOVERY_LIMIT.saturating_sub(released),
        )
        .await?;
        release_recovery = release_recovery.max(released + recovered);
        report.scenario_counts.cancellations = 1;
        report.result_counts.cancelled = 1;

        let (ttl, released) = renew_once(&provider).await?;
        update_ttl(&mut report.lease_summary, ttl);
        let recovered = wait_for_allocation_count(
            &shared,
            &context.base_url,
            baseline,
            RELEASE_RECOVERY_LIMIT.saturating_sub(released),
        )
        .await?;
        release_recovery = release_recovery.max(released + recovered);
        report.scenario_counts.renewals = 1;
        report.lease_summary.renewals_attempted = 1;
        report.lease_summary.renewals_succeeded = 1;

        let mut ttl_recovery_max = Duration::ZERO;
        for scenario in 0..3 {
            let (ttl, recovery) = match scenario {
                0 => (
                    None,
                    ttl_create_response_interruption(
                        &provider,
                        &shared,
                        &context.base_url,
                        baseline,
                    )
                    .await?,
                ),
                1 => {
                    let (ttl, recovery) =
                        ttl_release_interruption(&provider, &shared, &context.base_url, baseline)
                            .await?;
                    (Some(ttl), recovery)
                }
                _ => {
                    let (ttl, recovery) = ttl_client_exit_before_gateway(
                        &context,
                        &shared,
                        &context.base_url,
                        baseline,
                    )
                    .await?;
                    (Some(ttl), recovery)
                }
            };
            if let Some(ttl) = ttl {
                update_ttl(&mut report.lease_summary, ttl);
            }
            ttl_recovery_max = ttl_recovery_max.max(recovery);
            report.scenario_counts.ttl_recoveries += 1;
        }
        let final_count = metrics(&shared, &context.base_url).await?;
        if final_count != baseline {
            return Err(CanaryError::AllocationLeak);
        }
        report.resource_summary.baseline_active_allocations = baseline;
        report.resource_summary.max_active_allocations = baseline.saturating_add(1);
        report.resource_summary.final_active_allocations = final_count;
        report.timing_summary.release_recovery_max_ms =
            duration_millis_ceil(release_recovery).min(10_800_000);
        report.timing_summary.ttl_recovery_max_ms =
            duration_millis_ceil(ttl_recovery_max).min(10_800_000);
        Ok(())
    }
    .await;
    if let Err(error) = outcome {
        report.record_failure(&error);
    }
    report.finish(started)?;
    publish(&context, &report)
}

async fn observe_soak(
    expected_filename: &str,
    mode: &'static str,
    duration: Duration,
    planned_restart: bool,
) -> Result<(), CanaryError> {
    let context = context(expected_filename)?;
    let started = Instant::now();
    let mut report = ReadinessReport::new(&context, mode)?;
    let outcome: Result<(), CanaryError> = async {
        let shared = SharedLarmClient::build().map_err(|_| CanaryError::Metrics)?;
        let baseline = metrics(&shared, &context.base_url).await?;
        if baseline != 0 {
            return Err(CanaryError::AllocationLeak);
        }
        let mut maximum = baseline;
        let mut last_success = Instant::now();
        let mut maximum_gap = Duration::ZERO;
        let mut planned_gap = Duration::ZERO;
        let mut restart_failure_seen = false;
        let mut planned_gap_observed = false;
        let mut active_since: Option<Instant> = None;
        let mut release_recovery_max = Duration::ZERO;
        let mut samples = 1_u64;
        while started.elapsed() < duration {
            tokio::time::sleep(SAMPLE_INTERVAL).await;
            match metrics(&shared, &context.base_url).await {
                Ok(active) => {
                    let gap = last_success.elapsed();
                    if restart_failure_seen {
                        if gap > Duration::from_secs(120) {
                            return Err(CanaryError::Sampling);
                        }
                        planned_gap = planned_gap.max(gap);
                        restart_failure_seen = false;
                        planned_gap_observed = true;
                    } else if gap > Duration::from_secs(15) {
                        return Err(CanaryError::Sampling);
                    } else {
                        maximum_gap = maximum_gap.max(gap);
                    }
                    last_success = Instant::now();
                    maximum = maximum.max(active);
                    if active > baseline.saturating_add(1) {
                        return Err(CanaryError::AllocationLeak);
                    }
                    if active > baseline && active_since.is_none() {
                        active_since = Some(Instant::now());
                    } else if active == baseline {
                        if let Some(active_started) = active_since.take() {
                            release_recovery_max =
                                release_recovery_max.max(active_started.elapsed());
                            if release_recovery_max > RELEASE_RECOVERY_LIMIT {
                                return Err(CanaryError::Release);
                            }
                        }
                    }
                    samples += 1;
                    if samples > 4_096 {
                        return Err(CanaryError::Sampling);
                    }
                }
                Err(error) => {
                    if !matches!(&error, CanaryError::Sampling) {
                        return Err(error);
                    }
                    let in_restart_window = planned_restart
                        && started.elapsed() >= Duration::from_secs(30 * 60)
                        && started.elapsed() <= Duration::from_secs(40 * 60);
                    let continuing_planned_gap = restart_failure_seen && !planned_gap_observed;
                    if (!in_restart_window && !continuing_planned_gap)
                        || (planned_gap_observed && !restart_failure_seen)
                        || last_success.elapsed() > Duration::from_secs(120)
                    {
                        return Err(CanaryError::Sampling);
                    }
                    restart_failure_seen = true;
                }
            }
        }
        if planned_restart && planned_gap.is_zero() {
            return Err(CanaryError::Sampling);
        }
        let final_recovery =
            wait_for_allocation_count(&shared, &context.base_url, baseline, RELEASE_RECOVERY_LIMIT)
                .await?;
        if let Some(active_started) = active_since {
            release_recovery_max = release_recovery_max.max(active_started.elapsed());
        }
        report.resource_summary.baseline_active_allocations = baseline;
        report.resource_summary.max_active_allocations = maximum;
        report.resource_summary.final_active_allocations = baseline;
        report.timing_summary.sample_interval_seconds = SAMPLE_INTERVAL.as_secs();
        report.timing_summary.metrics_max_sampling_gap_seconds =
            duration_seconds_ceil(maximum_gap).min(10_800);
        report.timing_summary.planned_larm_restart_gap_seconds =
            duration_seconds_ceil(planned_gap).min(10_800);
        report.timing_summary.release_recovery_max_ms =
            duration_millis_ceil(release_recovery_max.max(final_recovery)).min(10_800_000);
        Ok(())
    }
    .await;
    if let Err(error) = outcome {
        report.record_failure(&error);
    }
    report.finish(started)?;
    publish(&context, &report)
}

#[tokio::test]
#[ignore = "operator-only read-only 30-minute LARM metrics observer"]
async fn observe_soak_30m() -> Result<(), CanaryError> {
    observe_soak(
        ".soak-30m-rust.json",
        "soak-30m",
        Duration::from_secs(30 * 60),
        false,
    )
    .await
}

#[tokio::test]
#[ignore = "operator-only read-only 2-hour LARM metrics observer"]
async fn observe_soak_2h() -> Result<(), CanaryError> {
    observe_soak(
        ".soak-2h-rust.json",
        "soak-2h",
        Duration::from_secs(2 * 60 * 60),
        true,
    )
    .await
}

#[test]
fn canary_prompt_catalog_matches_the_implementation_contract() {
    assert_eq!(
        CANARY_PROMPTS,
        [
            "Reply with exactly: CANARY_OK",
            "List the numbers 1 through 5.",
            "Write one short greeting in Japanese.",
            "Write ten numbered words, one at a time.",
            "Reply with exactly: READY",
        ]
    );
    assert!(CANARY_PROMPTS
        .iter()
        .all(|prompt| !prompt.is_empty() && prompt.len() <= 80));
}
