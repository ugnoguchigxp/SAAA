use super::agent_session::probe_agent_session_provider as probe_agent_session;
use super::{openai_compatible::probe_model_provider, stream::larm_failure_message};
use crate::persistence::validate_model_providers;
use crate::{
    redact::redact_runtime_text, validate_identifier, AppState, ModelProviderSettings,
    ModelProvidersSettings, ProviderTestResult, TestProviderInput,
};

type TestResult = Result<ProviderTestResult, String>;

pub(crate) async fn test_model_provider(state: &AppState, input: TestProviderInput) -> TestResult {
    let mut provider = input.provider;
    provider.set_enabled(true);
    validate_identifier(provider.id(), "provider id")?;
    validate_model_providers(&ModelProvidersSettings {
        harness: crate::HarnessSettings {
            address: String::new(),
        },
        providers: vec![provider.clone()],
        reasoning_effort: crate::providers::default_conversation_reasoning_effort(),
    })?;
    let captured_configuration = super::probe_state::capture_if_current(state, &provider);
    let started = std::time::Instant::now();
    let result = match &provider {
        ModelProviderSettings::OpenAiCompatible(provider) => probe_model_provider(provider).await,
        ModelProviderSettings::AgentSession(provider) => probe_agent_session(provider).await,
        ModelProviderSettings::CloudAsr(provider) => crate::voice::cloud_asr::probe(provider).await,
        ModelProviderSettings::CloudTts(provider) => crate::voice::cloud_tts::probe(provider).await,
        ModelProviderSettings::SystemTts(_) => Ok("System text-to-speech is available".to_string()),
        ModelProviderSettings::Larm(provider) => {
            crate::providers::larm::LarmProvider::probe(&state.larm_gate, &provider.base_url)
                .await
                .map(|_| "LARM health and readiness checks succeeded".to_string())
                .map_err(|kind| larm_failure_message(kind).to_string())
        }
        ModelProviderSettings::DynamicLan(provider) => {
            super::dynamic_lan::probe::probe(provider).await
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
