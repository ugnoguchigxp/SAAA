use crate::contract::{self, ContextWindow};

#[derive(Debug)]
pub struct CatalogProfile {
    pub revision: String,
    pub selector: String,
    pub id: String,
    pub providers: Vec<CatalogProvider>,
    pub services: Vec<CatalogService>,
}

#[derive(Debug)]
pub struct CatalogProvider {
    pub name: String,
    pub capability: String,
    pub protocol: String,
    pub endpoint: String,
    pub model: String,
    pub context_window: Option<ContextWindow>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CatalogService {
    pub name: String,
    pub capability: String,
    pub protocol: String,
    pub endpoint: String,
    pub model: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub startup_policy: Option<ServiceStartupPolicy>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ServiceStartupPolicy {
    pub min_warm_instances: u32,
    #[serde(flatten)]
    pub details: serde_json::Map<String, serde_json::Value>,
}

impl CatalogService {
    pub fn starts_on_request(&self) -> bool {
        self.startup_policy
            .as_ref()
            .is_some_and(|policy| policy.min_warm_instances == 0)
    }
}

impl CatalogProfile {
    pub fn provider(&self, name: &str) -> Option<&CatalogProvider> {
        self.providers.iter().find(|provider| provider.name == name)
    }
    pub fn service(&self, capability: &str) -> Option<&CatalogService> {
        self.services
            .iter()
            .find(|service| service.capability == capability)
    }
}

pub async fn fetch(
    client: &reqwest::Client,
    control_base: &url::Url,
    token: &str,
    selector: &str,
) -> Result<CatalogProfile, &'static str> {
    let mut url = control_base.clone();
    url.set_path("/v3/agent-profiles");
    url.query_pairs_mut()
        .clear()
        .append_pair("profile", selector);
    let value = match crate::http::json(crate::authorize(client.get(url), token)?, &[200]).await {
        Ok(value) => value,
        Err("larm_response_too_large") => return Err("larm_response_too_large"),
        Err("larm_invalid_json") => return Err("larm_catalog_invalid"),
        Err(_) => return Err("larm_catalog_unavailable"),
    };
    if value["contractVersion"] != "agent-connection.v3" {
        return Err("larm_catalog_unsupported");
    }
    let revision = required_text(&value, "catalogRevision")?;
    if value["requestedProfile"] != selector {
        return Err("larm_catalog_invalid");
    }
    let profiles = value["profiles"].as_array().ok_or("larm_catalog_invalid")?;
    if profiles.len() != 1 {
        return Err("larm_catalog_invalid");
    }
    let profile = &profiles[0];
    let id = required_text(profile, "id")?;
    let providers = profile["providers"]
        .as_array()
        .ok_or("larm_catalog_invalid")?
        .iter()
        .map(parse_provider)
        .collect::<Result<Vec<_>, _>>()?;
    let services = profile["services"]
        .as_array()
        .ok_or("larm_catalog_invalid")?
        .iter()
        .map(parse_service)
        .collect::<Result<Vec<_>, _>>()?;
    let mut names = std::collections::HashSet::new();
    for service in &services {
        if !names.insert(&service.name) {
            return Err("larm_catalog_invalid");
        }
        public_url(control_base, &service.endpoint)?;
    }
    Ok(CatalogProfile {
        revision,
        selector: selector.to_string(),
        id,
        providers,
        services,
    })
}

fn parse_provider(value: &serde_json::Value) -> Result<CatalogProvider, &'static str> {
    let context_window = if value.get("contextWindow").is_some() {
        Some(contract::context_window(value).map_err(|_| "larm_catalog_invalid")?)
    } else {
        None
    };
    Ok(CatalogProvider {
        name: required_text(value, "name")?,
        capability: required_text(value, "capability")?,
        protocol: required_text(value, "protocol")?,
        endpoint: required_text(value, "endpoint")?,
        model: required_text(value, "model")?,
        context_window,
    })
}

fn parse_service(value: &serde_json::Value) -> Result<CatalogService, &'static str> {
    Ok(CatalogService {
        name: required_text(value, "name")?,
        capability: required_text(value, "capability")?,
        protocol: required_text(value, "protocol")?,
        endpoint: required_text(value, "endpoint")?,
        model: required_text(value, "model")?,
        startup_policy: value
            .get("startupPolicy")
            .map(|policy| {
                serde_json::from_value(policy.clone()).map_err(|_| "larm_catalog_invalid")
            })
            .transpose()?,
    })
}

/// Control credentials are only sent to the public API's origin, never to a backend port.
pub fn public_url(base: &url::Url, endpoint: &str) -> Result<url::Url, &'static str> {
    if endpoint.starts_with("//") || endpoint.is_empty() {
        return Err("larm_invalid_service_endpoint");
    }
    let url = base
        .join(endpoint)
        .map_err(|_| "larm_invalid_service_endpoint")?;
    if url.origin() != base.origin()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err("larm_invalid_service_endpoint");
    }
    Ok(url)
}

fn required_text(value: &serde_json::Value, key: &str) -> Result<String, &'static str> {
    value[key]
        .as_str()
        .filter(|text| !text.is_empty() && text.len() <= 4096)
        .map(str::to_string)
        .ok_or("larm_catalog_invalid")
}
