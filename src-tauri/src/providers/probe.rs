use super::agent_session::probe_agent_session_provider as probe_agent_session;
use super::openai_compatible::probe_model_provider;
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
            larm_profile: None,
            tts_voice: None,
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        AgentSessionProviderSettings, CloudAsrProviderSettings, CloudTtsProviderSettings,
        SystemTtsProviderSettings,
    };
    use rusqlite::Connection;

    fn state() -> AppState {
        let connection = Connection::open_in_memory().expect("database opens");
        crate::initialize_database(&connection).expect("database initializes");
        crate::test_support::app_state(connection)
    }

    #[tokio::test]
    async fn provider_probes_cover_local_success_and_missing_credential_paths() {
        let state = state();
        let system = test_model_provider(
            &state,
            TestProviderInput {
                provider: ModelProviderSettings::SystemTts(SystemTtsProviderSettings {
                    id: "system-tts".into(),
                    enabled: false,
                    label: "System Voice".into(),
                    location: "local".into(),
                    voice: "default".into(),
                }),
            },
        )
        .await
        .expect("system TTS probe returns");
        assert!(system.ok);
        assert!(system.message.contains("System text-to-speech"));

        let mut openai = crate::test_support::direct_provider("cloud-probe", "cloud");
        openai.authentication = "api-key".into();
        let openai = test_model_provider(
            &state,
            TestProviderInput {
                provider: ModelProviderSettings::OpenAiCompatible(openai),
            },
        )
        .await
        .expect("openai probe returns");
        assert!(!openai.ok);
        assert!(openai.message.contains("API key"));

        let agent = test_model_provider(
            &state,
            TestProviderInput {
                provider: ModelProviderSettings::AgentSession(AgentSessionProviderSettings {
                    id: "agent-session".into(),
                    enabled: true,
                    label: "Agent Session".into(),
                    location: "cloud".into(),
                    base_url: "https://example.invalid/".into(),
                    model: "probe-model".into(),
                    models_path: "/v1/agents/models?runtime=fixture".into(),
                    sessions_path: "/v1/sessions".into(),
                    authentication: "api-key".into(),
                }),
            },
        )
        .await
        .expect("agent session probe returns");
        assert!(!agent.ok);

        let asr = test_model_provider(
            &state,
            TestProviderInput {
                provider: ModelProviderSettings::CloudAsr(CloudAsrProviderSettings {
                    id: "cloud-asr".into(),
                    enabled: true,
                    label: "Cloud ASR".into(),
                    location: "cloud".into(),
                    endpoint: "https://example.invalid/v1".into(),
                    model: "asr-model".into(),
                    language: "auto".into(),
                    authentication: "api-key".into(),
                }),
            },
        )
        .await
        .expect("cloud ASR probe returns");
        assert!(!asr.ok);

        let tts = test_model_provider(
            &state,
            TestProviderInput {
                provider: ModelProviderSettings::CloudTts(CloudTtsProviderSettings {
                    id: "cloud-tts".into(),
                    enabled: true,
                    label: "Cloud TTS".into(),
                    location: "cloud".into(),
                    endpoint: "https://example.invalid/v1".into(),
                    model: "tts-model".into(),
                    voice: "alloy".into(),
                    response_format: "wav".into(),
                    authentication: "api-key".into(),
                }),
            },
        )
        .await
        .expect("cloud TTS probe returns");
        assert!(!tts.ok);

        assert!(test_model_provider(
            &state,
            TestProviderInput {
                provider: ModelProviderSettings::SystemTts(SystemTtsProviderSettings {
                    id: "bad id".into(),
                    enabled: true,
                    label: "System Voice".into(),
                    location: "local".into(),
                    voice: "default".into(),
                }),
            },
        )
        .await
        .is_err());
    }
}
