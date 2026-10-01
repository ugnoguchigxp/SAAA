//! Keep Warm Provider contracts and Cold service declarations separate.
use super::{contract_error, ConnectionState, DynamicLanError, SelectedLlmProfile};
use std::collections::BTreeMap;

pub(super) fn validate_services(
    state: &ConnectionState,
    expected_profile: &SelectedLlmProfile,
) -> Result<(), DynamicLanError> {
    let services_value = serde_json::json!({"services": state.services});
    saaa_larm_session::validate_services(&services_value, &expected_profile.selector, None)
        .map_err(contract_error)?;
    if expected_profile.compare_catalog {
        if state.services.len() != expected_profile.catalog_services.len() {
            return Err(contract_error(()));
        }
        for declared in &expected_profile.catalog_services {
            if !state.services.iter().any(|service| {
                service["name"] == declared.name
                    && service["capability"] == declared.capability
                    && service["protocol"] == declared.protocol
                    && service["endpoint"] == declared.endpoint
                    && service["model"] == declared.model
            }) {
                return Err(contract_error(()));
            }
        }
    }
    Ok(())
}

type Models = BTreeMap<String, String>;
type Contracts = BTreeMap<String, (String, String)>;
pub(super) fn catalog_contracts(
    catalog: &saaa_larm_session::catalog::CatalogProfile,
) -> Result<(Models, Contracts), DynamicLanError> {
    saaa_larm_session::catalog_providers(catalog).map_err(contract_error)?;
    let mut catalog_models = std::collections::BTreeMap::new();
    let mut catalog_contracts = std::collections::BTreeMap::new();
    for entry in &catalog.providers {
        let expected = if entry.protocol == "larm.system-one.v1" {
            ("larm.system-one.v1", "/v1/systemone")
        } else {
            provider_contract(&entry.name).ok_or_else(|| contract_error(()))?
        };
        catalog_contracts.insert(
            entry.name.clone(),
            (entry.protocol.clone(), entry.endpoint.clone()),
        );
        if entry.protocol != expected.0
            || entry.endpoint != expected.1
            || entry.model.is_empty()
            || catalog_models
                .insert(entry.name.clone(), entry.model.clone())
                .is_some()
        {
            return Err(contract_error(()));
        }
    }
    Ok((catalog_models, catalog_contracts))
}

pub(super) fn provider_contract(name: &str) -> Option<(&'static str, &'static str)> {
    match name {
        "llm" | "backchannel" => Some(("openai.chat-completions.v1", "/v1/chat/completions")),
        "asr" => Some(("openai.audio-transcriptions.v1", "/v1/audio/transcriptions")),
        "tts" => Some(("openai.audio-speech.v1", "/v1/audio/speech")),
        "embedding" => Some(("larm.embedding.v1", "/v1/embed")),
        _ => None,
    }
}
