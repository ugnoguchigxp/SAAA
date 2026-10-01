//! Operator-registered execution policy. A private URL alone grants no local-only proof.
use serde::Deserialize;
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LocalBinding {
    pub endpoint: String,
    pub runtime: String,
    pub model: String,
    pub execution: String,
    pub cloud_forwarding: bool,
}
impl LocalBinding {
    pub fn load() -> Result<Self, String> {
        let raw = std::env::var("SAAA_PERSONAL_STATE_LOCAL_BINDING")
            .map_err(|_| "personal-local-binding-unverified")?;
        Self::parse(&raw)
    }
    fn parse(raw: &str) -> Result<Self, String> {
        if raw.len() > 2048 {
            return Err("personal-local-binding-unverified".into());
        }
        let value: Self =
            serde_json::from_str(raw).map_err(|_| "personal-local-binding-unverified")?;
        if value.execution != "user_managed_local"
            || value.cloud_forwarding
            || value.runtime.is_empty()
            || value.model.is_empty()
        {
            return Err("personal-local-binding-unverified".into());
        }
        let url =
            url::Url::parse(&value.endpoint).map_err(|_| "personal-local-binding-unverified")?;
        if !saaa_larm_session::local_url(&url)
            || !url.username().is_empty()
            || url.password().is_some()
            || url.path() != "/"
            || url.query().is_some()
            || url.fragment().is_some()
            || value.endpoint != url.origin().ascii_serialization()
        {
            return Err("personal-local-binding-unverified".into());
        }
        Ok(value)
    }
    pub fn validate(&self, endpoint: &str, runtime: &str, model: &str) -> Result<(), String> {
        if (
            self.endpoint.as_str(),
            self.runtime.as_str(),
            self.model.as_str(),
        ) != (endpoint, runtime, model)
        {
            return Err("personal-local-binding-mismatch".into());
        }
        Ok(())
    }
}

pub(crate) fn maintenance_profile(
    value: Option<&str>,
) -> Result<saaa_larm_session::ProfilePreference, String> {
    match value {
        None => Ok(crate::providers::larm_resources::profile::preference(None)),
        Some(value) if !value.trim().is_empty() && value == value.trim() && value.len() <= 160 => {
            Ok(saaa_larm_session::ProfilePreference::Explicit(value.into()))
        }
        Some(_) => Err("personal-maintenance-profile-invalid".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn world_maintenance_local_policy_rejects_cloud_forwarding_and_mismatched_allocation() {
        let raw = r#"{"endpoint":"http://127.0.0.1:9810","runtime":"local-worker","model":"qwen-local","execution":"user_managed_local","cloud_forwarding":false}"#;
        let binding = LocalBinding::parse(raw).unwrap();
        binding
            .validate("http://127.0.0.1:9810", "local-worker", "qwen-local")
            .unwrap();
        assert!(binding
            .validate("http://127.0.0.1:9810", "local-worker", "other")
            .is_err());
        assert!(LocalBinding::parse(&raw.replace("false", "true")).is_err());
        assert!(LocalBinding::parse(
            &raw.replace("http://127.0.0.1:9810", "https://api.example.com")
        )
        .is_err());
        assert!(LocalBinding::parse(&raw.replace("user_managed_local", "cloud")).is_err());
    }
}
