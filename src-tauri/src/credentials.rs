use serde::{Deserialize, Serialize};
use std::sync::Mutex;
use zeroize::{Zeroize, Zeroizing};

pub(crate) const PROVIDER_CREDENTIAL_SERVICE: &str = "com.saaa.provider-api-key";
const MAX_CREDENTIAL_BYTES: usize = 2_560;
static CREDENTIAL_STORE: Mutex<()> = Mutex::new(());

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

#[allow(dead_code)] // Platform credential write API is not exercised on every test target.
pub(crate) fn store_named_secret(service: &str, account: &str, value: &[u8]) -> Result<(), String> {
    validate_named_secret(service, account, value)?;
    with_entry(service, account, |entry| {
        entry.set_secret(value).map_err(|_| {
            "Could not store the credential in the operating system credential store".into()
        })
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
    with_entry(service, account, |entry| match entry.get_secret() {
        Ok(value) => match String::from_utf8(value) {
            Ok(value) => Ok(Some(Zeroizing::new(value))),
            Err(error) => {
                let mut value = error.into_bytes();
                value.zeroize();
                Err(
                    "The credential stored in the operating system credential store is invalid"
                        .into(),
                )
            }
        },
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(_) => {
            Err("Could not read the credential from the operating system credential store".into())
        }
    })
}

pub(crate) fn delete_named_secret(service: &str, account: &str) -> Result<(), String> {
    with_entry(service, account, |entry| match entry.delete_credential() {
        Ok(()) => Ok(()),
        Err(keyring::Error::NoEntry) => Ok(()),
        Err(_) => {
            Err("Could not delete the credential from the operating system credential store".into())
        }
    })
}

fn with_entry<T>(
    service: &str,
    account: &str,
    operation: impl FnOnce(&keyring::Entry) -> Result<T, String>,
) -> Result<T, String> {
    let _guard = CREDENTIAL_STORE
        .lock()
        .map_err(|_| "Operating system credential store lock unavailable".to_string())?;
    let entry = keyring::Entry::new(service, account)
        .map_err(|_| "Operating system credential store unavailable".to_string())?;
    operation(&entry)
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
