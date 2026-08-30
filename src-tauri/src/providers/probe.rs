use std::sync::Arc;

use super::openai_compatible::probe_model_provider;
use super::stream::larm_failure_message;
use crate::persistence::validate_model_providers;
use crate::redact::redact_runtime_text;
use crate::{
    validate_identifier, AppState, ModelProviderSettings, ModelProvidersSettings,
    ProviderTestResult, RunCancellation, TestProviderInput,
};

pub(crate) async fn test_model_provider(
    state: &AppState,
    input: TestProviderInput,
) -> Result<ProviderTestResult, String> {
    let mut provider = input.provider;
    provider.set_enabled(true);
    validate_identifier(provider.id(), "provider id")?;
    validate_model_providers(&ModelProvidersSettings {
        providers: vec![provider.clone()],
        reasoning_effort: crate::providers::default_conversation_reasoning_effort(),
        max_output_tokens: crate::providers::completion::DEFAULT_MAX_OUTPUT_TOKENS,
    })?;
    let captured_configuration = super::probe_state::capture_if_current(state, &provider);
    let started = std::time::Instant::now();
    let result = match &provider {
        ModelProviderSettings::OpenAiCompatible(provider) => probe_model_provider(provider).await,
        ModelProviderSettings::Larm(provider) => {
            crate::providers::larm::LarmProvider::probe(&state.larm_gate, &provider.base_url)
                .await
                .map(|_| "LARM health and readiness checks succeeded".to_string())
                .map_err(|kind| larm_failure_message(kind).to_string())
        }
        ModelProviderSettings::DynamicLan(provider) => {
            match crate::providers::dynamic_lan::DynamicLanConnection::resolve(
                &provider.host,
                Arc::new(RunCancellation::default()),
            )
            .await
            {
                Ok(connection) => {
                    let message = format!(
                        "dynamic_lan dynamically resolved model {} at {}",
                        connection.model(),
                        connection.endpoint()
                    );
                    connection
                        .release()
                        .await
                        .map(|_| message)
                        .map_err(|error| error.public_message().to_string())
                }
                Err(error) => Err(error.public_message().to_string()),
            }
        }
    };
    let tested = ProviderTestResult {
        provider_id: provider.id().to_string(),
        ok: result.is_ok(),
        message: result.unwrap_or_else(|error| redact_runtime_text(&error)),
        latency_ms: started.elapsed().as_millis(),
    };
    super::probe_state::record_if_current(state, &provider, captured_configuration, tested.ok);
    Ok(tested)
}
