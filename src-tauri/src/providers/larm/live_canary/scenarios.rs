use super::super::{
    client::{Cancellation, ChatMessage, CleanupResult, SharedLarmClient},
    contracts::SessionFailureKind,
    AllocationCleanup, LarmProvider,
};
use std::{
    env, fs,
    path::PathBuf,
    process::Stdio,
    sync::atomic::Ordering,
    time::{Duration, Instant},
};
use tokio::process::Command;

use super::*;

pub(super) async fn normal_turn(
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

pub(super) async fn cancelled_turn(
    provider: &LarmProvider<'_>,
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

pub(super) async fn renew_once(
    provider: &LarmProvider<'_>,
) -> Result<(u32, Duration), CanaryError> {
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

pub(super) async fn wait_for_ttl_recovery(
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

pub(super) fn ttl_wait_error(error: CanaryError) -> CanaryError {
    if matches!(&error, CanaryError::AllocationLeak) {
        CanaryError::Ttl
    } else {
        error
    }
}

pub(super) async fn ttl_create_response_interruption(
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

pub(super) async fn ttl_release_interruption(
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

pub(super) async fn ttl_client_exit_helper() -> Result<(), CanaryError> {
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

pub(super) async fn ttl_client_exit_before_gateway(
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

pub(super) fn update_ttl(summary: &mut LeaseSummary, ttl: u32) {
    let ttl = u64::from(ttl);
    summary.effective_ttl_seconds_min = if summary.effective_ttl_seconds_min == 0 {
        ttl
    } else {
        summary.effective_ttl_seconds_min.min(ttl)
    };
    summary.effective_ttl_seconds_max = summary.effective_ttl_seconds_max.max(ttl);
}
