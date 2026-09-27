use super::*;
pub(crate) async fn resolve(
    provider: &DynamicLanProviderSettings,
    stored_profile: Option<&str>,
    timeout_ms: u64,
    cancellation: Arc<RunCancellation>,
) -> Result<
    (
        crate::providers::dynamic_lan::DynamicLanConnection,
        CleanupOutcome,
    ),
    DynamicLanConnectionFailure,
> {
    let local_cancel = Arc::new(RunCancellation::default());
    let provider = provider.clone();
    let stored_profile = stored_profile.map(str::to_string);
    let task_cancel = local_cancel.clone();
    let mut task = tokio::spawn(async move {
        resolve_connection(
            &provider,
            stored_profile.as_deref(),
            timeout_ms,
            task_cancel,
        )
        .await
    });
    let timed_out = tokio::select! { biased;
        _ = cancellation.cancelled() => false,
        _ = tokio::time::sleep(Duration::from_millis(timeout_ms)) => true,
        result = &mut task => return result.unwrap_or_else(|_| Err(DynamicLanConnectionFailure { error: crate::providers::dynamic_lan::DynamicLanError::new(crate::providers::dynamic_lan::ErrorKind::Internal, "Provider initialization stopped"), cleanup: CleanupOutcome::DynamicLanDeferredToTtl {kind:"internal"} })),
    };
    local_cancel.cancel();
    // Keep ownership until a late create/claim response can be released.
    tokio::spawn(async move {
        if let Ok(Ok((connection, _))) = task.await {
            let _ = connection.release().await;
        }
    });
    Err(DynamicLanConnectionFailure {
        error: crate::providers::dynamic_lan::DynamicLanError::new(
            if timed_out {
                crate::providers::dynamic_lan::ErrorKind::Timeout
            } else {
                crate::providers::dynamic_lan::ErrorKind::Cancelled
            },
            "Provider initialization stopped",
        ),
        cleanup: CleanupOutcome::DynamicLanDeferredToTtl {
            kind: if timed_out { "timeout" } else { "internal" },
        },
    })
}
pub(super) async fn release_in_background(
    connection: crate::providers::dynamic_lan::DynamicLanConnection,
) -> CleanupOutcome {
    let mut task = tokio::spawn(async move { connection.release().await });
    match tokio::time::timeout(Duration::from_millis(250), &mut task).await {
        Ok(Ok(result)) => dynamic_lan_cleanup_from_release(result),
        _ => CleanupOutcome::DynamicLanDeferredToTtl { kind: "timeout" },
    }
}
