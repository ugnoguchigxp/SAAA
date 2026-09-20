//! Host-provider generators. The conversation generator observes the current routing settings.

use async_trait::async_trait;

use super::super::contracts::GenerationErrorCode;
use super::{GenerationPrompt, Generator};

pub(crate) struct ConversationProviderGenerator {
    writer: std::sync::Arc<crate::persistence::SqliteWriter>,
}

impl ConversationProviderGenerator {
    pub(crate) fn new(writer: std::sync::Arc<crate::persistence::SqliteWriter>) -> Self {
        Self { writer }
    }
}

#[async_trait]
impl Generator for ConversationProviderGenerator {
    async fn generate(
        &self,
        prompt: &GenerationPrompt,
        cancellation: &crate::RunCancellation,
    ) -> Result<String, GenerationErrorCode> {
        let provider =
            conversation_provider(&self.writer).ok_or(GenerationErrorCode::Unavailable)?;
        crate::providers::openai_compatible::complete_generation(
            &provider,
            &prompt.system,
            &prompt.user,
            cancellation,
        )
        .await
        .map_err(|_| GenerationErrorCode::ModelError)
    }
}

pub(crate) fn conversation_provider(
    writer: &crate::persistence::SqliteWriter,
) -> Option<crate::OpenAiCompatibleProviderSettings> {
    writer
        .read_serialized(|connection| {
            let providers = crate::persistence::load_model_providers(connection)
                .map_err(|error| error.to_string())?;
            let route = crate::persistence::load_routing_settings(connection)
                .map_err(|error| error.to_string())?
                .conversation_respond;
            if route.source == "harness" {
                return Ok(None);
            }
            let configured = providers.providers.iter().find(|provider| {
                Some(provider.id()) == route.primary_provider_id.as_deref() && provider.enabled()
            });
            Ok(configured.and_then(|provider| match provider {
                crate::ModelProviderSettings::OpenAiCompatible(settings) => Some(settings.clone()),
                _ => None,
            }))
        })
        .ok()
        .flatten()
}
