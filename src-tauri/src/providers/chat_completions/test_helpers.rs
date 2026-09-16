#![cfg(test)]
use super::*;
pub(crate) async fn run(
    endpoint: &str,
    authorization: Option<&str>,
    model: &str,
    history: &[ConversationMessage],
    timeout_ms: u64,
    context: ModelStreamContext<'_>,
) -> Result<String, Error> {
    run_mode(
        endpoint,
        authorization,
        model,
        history,
        timeout_ms,
        context,
        RequestMode::Stream,
    )
    .await
}

pub(crate) async fn run_mode(
    endpoint: &str,
    authorization: Option<&str>,
    model: &str,
    history: &[ConversationMessage],
    timeout_ms: u64,
    context: ModelStreamContext<'_>,
    mode: RequestMode,
) -> Result<String, Error> {
    run_with_options(
        endpoint,
        authorization,
        model,
        history,
        timeout_ms,
        context,
        mode,
        &saaa_larm_session::http_api::LlmOptions::standard(),
    )
    .await
}
