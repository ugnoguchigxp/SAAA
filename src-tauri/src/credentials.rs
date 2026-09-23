use rusqlite::params;
use serde::{Deserialize, Serialize};
use std::sync::{Arc, OnceLock};
use zeroize::Zeroizing;

pub(crate) const PROVIDER_CREDENTIAL_SERVICE: &str = "com.saaa.provider-api-key";
const MAX_CREDENTIAL_BYTES: usize = 2_560;
static CREDENTIAL_DATABASE: OnceLock<Arc<crate::persistence::SqliteWriter>> = OnceLock::new();

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct SetProviderApiKeyInput {
    pub(crate) provider_id: String,
    pub(crate) api_key: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ProviderCredentialState {
    pub(crate) provider_id: String,
    pub(crate) state: &'static str,
}

fn validate_provider_id(provider_id: &str) -> Result<(), String> {
    if provider_id.is_empty()
        || provider_id.len() > 80
        || !provider_id
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_'))
    {
        return Err("Provider id is invalid".to_string());
    }
    Ok(())
}

fn validate_api_key(api_key: &str) -> Result<(), String> {
    if api_key.is_empty()
        || api_key.len() > MAX_CREDENTIAL_BYTES
        || api_key.trim() != api_key
        || !api_key
            .bytes()
            .all(|byte| byte.is_ascii_graphic() && !matches!(byte, b'\r' | b'\n'))
    {
        return Err(
            "API key must contain 1–2560 visible ASCII characters without surrounding whitespace"
                .to_string(),
        );
    }
    Ok(())
}

pub(crate) fn set_api_key(
    readers: &crate::persistence::SqliteReaders,
    input: SetProviderApiKeyInput,
) -> Result<ProviderCredentialState, String> {
    let SetProviderApiKeyInput {
        provider_id,
        api_key,
    } = input;
    let api_key = Zeroizing::new(api_key);
    validate_provider_id(&provider_id)?;
    validate_api_key(&api_key)?;
    let providers = readers.read(crate::persistence::load_model_providers)?;
    if !provider_accepts_api_key(&providers, &provider_id) {
        return Err("API keys can be stored only for a saved API-key provider".to_string());
    }
    store_named_secret(
        PROVIDER_CREDENTIAL_SERVICE,
        &provider_id,
        api_key.as_bytes(),
    )?;
    Ok(ProviderCredentialState {
        provider_id,
        state: "configured",
    })
}

fn provider_accepts_api_key(providers: &crate::ModelProvidersSettings, provider_id: &str) -> bool {
    providers.providers.iter().any(|provider| match provider {
        crate::ModelProviderSettings::OpenAiCompatible(provider) => {
            provider.id == provider_id && provider.authentication == "api-key"
        }
        crate::ModelProviderSettings::AgentSession(provider) => {
            provider.id == provider_id && provider.authentication == "api-key"
        }
        crate::ModelProviderSettings::CloudAsr(provider) => {
            provider.id == provider_id && provider.authentication == "api-key"
        }
        crate::ModelProviderSettings::CloudTts(provider) => {
            provider.id == provider_id && provider.authentication == "api-key"
        }
        _ => false,
    })
}

pub(crate) fn delete_api_key(provider_id: String) -> Result<ProviderCredentialState, String> {
    validate_provider_id(&provider_id)?;
    delete_named_secret(PROVIDER_CREDENTIAL_SERVICE, &provider_id)?;
    Ok(ProviderCredentialState {
        provider_id,
        state: "missing",
    })
}

pub(crate) fn credential_state(provider_id: String) -> Result<ProviderCredentialState, String> {
    validate_provider_id(&provider_id)?;
    let state = if load_api_key(&provider_id)?.is_some() {
        "configured"
    } else {
        "missing"
    };
    Ok(ProviderCredentialState { provider_id, state })
}

pub(crate) fn install_database(writer: Arc<crate::persistence::SqliteWriter>) {
    let _ = CREDENTIAL_DATABASE.set(writer);
}

#[allow(dead_code)] // Platform credential write API is not exercised on every test target.
pub(crate) fn store_named_secret(service: &str, account: &str, value: &[u8]) -> Result<(), String> {
    validate_named_secret(service, account, value)?;
    let secret = std::str::from_utf8(value).map_err(|_| credential_error())?;
    database()?.write(|connection| {
        connection
            .execute(
                "INSERT INTO credential_secrets(service, account, secret, updated_at)
                 VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(service, account) DO UPDATE SET
                   secret = excluded.secret,
                   updated_at = excluded.updated_at",
                params![service, account, secret, crate::now_iso()],
            )
            .map_err(|_| credential_error())?;
        Ok(())
    })
}

fn validate_named_secret(service: &str, account: &str, value: &[u8]) -> Result<(), String> {
    if service.is_empty() || account.is_empty() {
        return Err("Credential service and account must not be empty".into());
    }
    if value.is_empty() || value.len() > MAX_CREDENTIAL_BYTES {
        return Err("Credential must contain 1–2560 bytes".into());
    }
    Ok(())
}

pub(crate) fn load_api_key(provider_id: &str) -> Result<Option<Zeroizing<String>>, String> {
    load_named_secret(PROVIDER_CREDENTIAL_SERVICE, provider_id)
}

pub(crate) fn load_named_secret(
    service: &str,
    account: &str,
) -> Result<Option<Zeroizing<String>>, String> {
    database()?.read_serialized(|connection| {
        match connection.query_row(
            "SELECT secret FROM credential_secrets WHERE service = ?1 AND account = ?2",
            params![service, account],
            |row| row.get::<_, String>(0),
        ) {
            Ok(secret) => Ok(Some(Zeroizing::new(secret))),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(_) => Err(credential_error()),
        }
    })
}

pub(crate) fn delete_named_secret(service: &str, account: &str) -> Result<(), String> {
    database()?.write(|connection| {
        connection
            .execute(
                "DELETE FROM credential_secrets WHERE service = ?1 AND account = ?2",
                params![service, account],
            )
            .map_err(|_| credential_error())?;
        Ok(())
    })
}

fn database() -> Result<Arc<crate::persistence::SqliteWriter>, String> {
    CREDENTIAL_DATABASE
        .get()
        .cloned()
        .ok_or_else(credential_error)
}

fn credential_error() -> String {
    "Could not use the credential stored in the application database".into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn api_key_validation_rejects_empty_whitespace_control_and_oversize_values() {
        assert!(validate_api_key("").is_err());
        assert!(validate_api_key(" secret").is_err());
        assert!(validate_api_key("secret\nheader").is_err());
        assert!(validate_api_key(&"x".repeat(MAX_CREDENTIAL_BYTES + 1)).is_err());
        assert!(validate_api_key("sk-valid_123").is_ok());
    }

    #[test]
    fn named_secret_validation_matches_the_windows_credential_limit() {
        assert!(validate_named_secret("service", "account", b"secret").is_ok());
        assert!(validate_named_secret("", "account", b"secret").is_err());
        assert!(validate_named_secret("service", "", b"secret").is_err());
        assert!(validate_named_secret("service", "account", b"").is_err());
        assert!(
            validate_named_secret("service", "account", &vec![b'x'; MAX_CREDENTIAL_BYTES + 1])
                .is_err()
        );
    }

    #[test]
    fn only_a_saved_api_key_provider_accepts_a_credential() {
        let mut settings = crate::ModelProvidersSettings {
            harness: crate::HarnessSettings {
                larm_profile: None,
                tts_voice: None,
                tts_style: None,
                tts_speed: None,
                tts_pitch_scale: None,
                tts_intonation_scale: None,
                address: String::new(),
            },
            providers: vec![crate::test_support::provider("cloud", "cloud")],
            reasoning_effort: "medium".to_string(),
        };
        if let crate::ModelProviderSettings::OpenAiCompatible(provider) = &mut settings.providers[0]
        {
            provider.authentication = "api-key".to_string();
        }
        assert!(provider_accepts_api_key(&settings, "cloud"));
        assert!(!provider_accepts_api_key(&settings, "missing"));
        if let crate::ModelProviderSettings::OpenAiCompatible(provider) = &mut settings.providers[0]
        {
            provider.authentication = "none".to_string();
        }
        assert!(!provider_accepts_api_key(&settings, "cloud"));
    }
}
