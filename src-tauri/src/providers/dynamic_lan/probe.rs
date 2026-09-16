use crate::RunCancellation;
use std::sync::Arc;

pub(crate) async fn probe(provider: &crate::DynamicLanProviderSettings) -> Result<String, String> {
    match crate::providers::dynamic_lan::DynamicLanConnection::resolve(
        &provider.host,
        Arc::new(RunCancellation::default()),
    )
    .await
    {
        Ok(connection) => {
            let resolved = crate::OpenAiCompatibleProviderSettings {
                request_options: provider.request_options.clone(),
                id: provider.id.clone(),
                enabled: true,
                label: provider.label.clone(),
                location: "local".into(),
                endpoint: connection.endpoint().into(),
                model: connection.model().into(),
                authentication: if connection.api_key().is_some() {
                    "api-key"
                } else {
                    "none"
                }
                .into(),
            };
            let result = crate::providers::openai_compatible::probe_model_provider_with_api_key(
                &resolved,
                connection.api_key(),
            )
            .await;
            let release = connection
                .release()
                .await
                .map_err(|error| error.public_message().to_string());
            release?;
            result
        }
        Err(error) => Err(error.public_message().to_string()),
    }
}
