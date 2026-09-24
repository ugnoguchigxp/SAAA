use crate::contract::{required_providers, CANONICAL_PROFILE, LEGACY_PROFILE, PREVIOUS_DEFAULT_PROFILE};
use serde_json::Value;

pub struct CatalogProfile {
    pub id: String,
    pub provider_names: Vec<String>,
}

pub(crate) async fn fetch(
    client: &reqwest::Client,
    control_base: &url::Url,
    token: &str,
) -> Result<Vec<CatalogProfile>, &'static str> {
    let mut url = control_base.clone();
    url.set_path("/v3/agent-profiles");
    let value = match crate::http::json(crate::authorize(client.get(url), token)?, &[200]).await {
        Ok(value) => value,
        Err("larm_response_too_large") => return Err("larm_response_too_large"),
        Err("larm_invalid_json") => return Err("larm_catalog_invalid"),
        Err(_) => return Err("larm_catalog_unavailable"),
    };
    if value["contractVersion"] != "agent-connection.v3" {
        return Err("larm_catalog_unsupported");
    }
    let profiles = value["profiles"]
        .as_array()
        .ok_or("larm_catalog_invalid")?;
    profiles
        .iter()
        .map(|profile| {
            let id = profile["id"]
                .as_str()
                .filter(|id| !id.is_empty())
                .ok_or("larm_catalog_invalid")?
                .to_string();
            let names = profile["providers"]
                .as_array()
                .ok_or("larm_catalog_invalid")?
                .iter()
                .map(|provider| {
                    provider["name"]
                        .as_str()
                        .filter(|name| !name.is_empty())
                        .map(str::to_string)
                        .ok_or("larm_catalog_invalid")
                })
                .collect::<Result<Vec<_>, _>>()?;
            Ok(CatalogProfile {
                id,
                provider_names: names,
            })
        })
        .collect()
}

pub fn select_voice_profile(profiles: &[CatalogProfile]) -> Result<String, &'static str> {
    for id in [CANONICAL_PROFILE, LEGACY_PROFILE, PREVIOUS_DEFAULT_PROFILE] {
        if profiles.iter().filter(|profile| profile.id == id).count() > 1 {
            return Err("larm_catalog_invalid");
        }
    }
    for id in [CANONICAL_PROFILE, LEGACY_PROFILE, PREVIOUS_DEFAULT_PROFILE] {
        if let Some(profile) = profiles.iter().find(|profile| profile.id == id) {
            let required = required_providers(id);
            if required
                .iter()
                .all(|name| profile.provider_names.iter().any(|present| present == name))
            {
                return Ok((*id).to_string());
            }
        }
    }
    Err("larm_profile_unavailable")
}
