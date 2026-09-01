use std::sync::Arc;
use std::time::Duration;

use super::attempt::*;
use super::{stream_model_provider_with_api_key, ModelStreamContext};
use crate::ipc_contract::ConversationMessage;
use crate::{DynamicLanProviderSettings, OpenAiCompatibleProviderSettings, RunCancellation};

pub(crate) async fn stream_dynamic_lan_provider(
    provider: &DynamicLanProviderSettings,
    history: &[ConversationMessage],
    timeout_ms: u64,
    cancellation: Arc<RunCancellation>,
    context: ModelStreamContext<'_>,
) -> ProviderAttemptOutcome {
    let (connection, prior_cleanup) = match resolve_dynamic_lan_connection_for_request(
        provider,
        timeout_ms,
        cancellation.clone(),
    )
    .await
    {
        Ok(connection) => connection,
        Err(failure) => {
            let kind = provider_failure_from_dynamic_lan(failure.error.kind);
            return if kind == ProviderFailureKind::Cancelled {
                ProviderAttemptOutcome::Cancelled {
                    output_started: false,
                    cleanup: failure.cleanup,
                }
            } else {
                ProviderAttemptOutcome::Failed {
                    kind,
                    public_message: kind.public_message(),
                    output_started: false,
                    cleanup: failure.cleanup,
                }
            };
        }
    };
    let resolved = OpenAiCompatibleProviderSettings {
        id: provider.id.clone(),
        enabled: true,
        label: provider.label.clone(),
        location: "local".to_string(),
        endpoint: connection.stream_url().to_string(),
        model: connection.model().to_string(),
        authentication: if connection.api_key().is_some() {
            "api-key"
        } else {
            "none"
        }
        .to_string(),
    };
    let outcome = stream_model_provider_with_api_key(
        &resolved,
        history,
        timeout_ms,
        connection.api_key(),
        Some(connection.allocation_id()),
        context,
    )
    .await;
    let cleanup = merge_dynamic_lan_cleanup(
        prior_cleanup,
        dynamic_lan_cleanup_from_release(connection.release().await),
    );
    outcome.with_cleanup(cleanup)
}

pub(crate) struct DynamicLanConnectionFailure {
    pub(crate) error: crate::providers::dynamic_lan::DynamicLanError,
    pub(crate) cleanup: CleanupOutcome,
}

pub(crate) fn dynamic_lan_cleanup_from_release(
    release: Result<(), crate::providers::dynamic_lan::DynamicLanError>,
) -> CleanupOutcome {
    match release {
        Ok(()) => CleanupOutcome::Released,
        Err(error) => CleanupOutcome::DynamicLanDeferredToTtl {
            kind: dynamic_lan_release_failure_kind(error.kind),
        },
    }
}

pub(crate) fn merge_dynamic_lan_cleanup(
    previous: CleanupOutcome,
    current: CleanupOutcome,
) -> CleanupOutcome {
    match (previous, current) {
        (deferred @ CleanupOutcome::DynamicLanDeferredToTtl { .. }, _) => deferred,
        (_, deferred @ CleanupOutcome::DynamicLanDeferredToTtl { .. }) => deferred,
        (CleanupOutcome::Released, _) | (_, CleanupOutcome::Released) => CleanupOutcome::Released,
        _ => CleanupOutcome::NotStarted,
    }
}

pub(crate) fn dynamic_lan_release_failure_kind(
    kind: crate::providers::dynamic_lan::ErrorKind,
) -> &'static str {
    use crate::providers::dynamic_lan::ErrorKind as DynamicLan;
    match kind {
        DynamicLan::Authentication => "authentication",
        DynamicLan::Network => "network",
        DynamicLan::Timeout => "timeout",
        DynamicLan::Upstream | DynamicLan::Unavailable | DynamicLan::Capacity => "upstream",
        DynamicLan::Contract | DynamicLan::StaleConnection => "protocol",
        DynamicLan::Cancelled | DynamicLan::Internal => "internal",
    }
}

pub(crate) async fn resolve_dynamic_lan_connection_for_request(
    provider: &DynamicLanProviderSettings,
    timeout_ms: u64,
    cancellation: Arc<RunCancellation>,
) -> Result<
    (
        crate::providers::dynamic_lan::DynamicLanConnection,
        CleanupOutcome,
    ),
    DynamicLanConnectionFailure,
> {
    let mut cleanup = CleanupOutcome::NotStarted;
    for attempt in 0..2 {
        let mut connection = match crate::providers::dynamic_lan::DynamicLanConnection::resolve(
            &provider.host,
            cancellation.clone(),
        )
        .await
        {
            Ok(connection) => {
                cleanup = merge_dynamic_lan_cleanup(
                    cleanup,
                    dynamic_lan_cleanup_from_release_failure(connection.prior_release_failure()),
                );
                connection
            }
            Err(error) => {
                cleanup = merge_dynamic_lan_cleanup(
                    cleanup,
                    dynamic_lan_cleanup_from_release_failure(error.release_failure()),
                );
                return Err(DynamicLanConnectionFailure { error, cleanup });
            }
        };
        match connection
            .ensure_lifetime(Duration::from_millis(timeout_ms), cancellation.clone())
            .await
        {
            Ok(()) => return Ok((connection, cleanup)),
            Err(error)
                if error.kind == crate::providers::dynamic_lan::ErrorKind::StaleConnection
                    && attempt == 0 =>
            {
                cleanup = merge_dynamic_lan_cleanup(
                    cleanup,
                    dynamic_lan_cleanup_from_release(connection.release().await),
                );
            }
            Err(error) => {
                cleanup = merge_dynamic_lan_cleanup(
                    cleanup,
                    dynamic_lan_cleanup_from_release(connection.release().await),
                );
                return Err(DynamicLanConnectionFailure { error, cleanup });
            }
        }
    }
    Err(DynamicLanConnectionFailure {
        error: crate::providers::dynamic_lan::DynamicLanError::new(
            crate::providers::dynamic_lan::ErrorKind::StaleConnection,
            "The dynamic LAN provider connection expired before inference started.",
        ),
        cleanup,
    })
}

pub(crate) fn dynamic_lan_cleanup_from_release_failure(
    kind: Option<crate::providers::dynamic_lan::ErrorKind>,
) -> CleanupOutcome {
    kind.map_or(CleanupOutcome::NotStarted, |kind| {
        CleanupOutcome::DynamicLanDeferredToTtl {
            kind: dynamic_lan_release_failure_kind(kind),
        }
    })
}
