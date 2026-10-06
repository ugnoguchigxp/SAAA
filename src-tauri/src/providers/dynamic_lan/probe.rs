use crate::RunCancellation;
use std::sync::Arc;
use std::time::Duration;
use url::Url;

/// Same check as a live Agent Connection: claim, then the model readiness probe.
/// A chat-completion request is not part of reachability.
pub(crate) async fn probe(provider: &crate::DynamicLanProviderSettings) -> Result<String, String> {
    let connection = crate::providers::dynamic_lan::DynamicLanConnection::resolve(
        &provider.host,
        None,
        Arc::new(RunCancellation::default()),
    )
    .await
    .map_err(|error| error.public_message().to_string())?;
    connection
        .release()
        .await
        .map_err(|error| error.public_message().to_string())?;
    Ok("Agent connection and model readiness probe succeeded.".to_string())
}
