use serde::{Deserialize, Serialize};
use zeroize::{Zeroize, Zeroizing};

const KEYCHAIN_SERVICE: &str = "com.saaa.provider-api-key";
const KEYCHAIN_NOT_FOUND: i32 = -25_300;

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
        || api_key.len() > 4_096
        || api_key.trim() != api_key
        || !api_key
            .bytes()
            .all(|byte| byte.is_ascii_graphic() && !matches!(byte, b'\r' | b'\n'))
    {
        return Err(
            "API key must contain 1–4096 visible ASCII characters without surrounding whitespace"
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
    set_keychain_value(&provider_id, api_key.as_bytes())?;
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
    delete_keychain_value(&provider_id)?;
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

#[cfg(target_os = "macos")]
fn set_keychain_value(provider_id: &str, value: &[u8]) -> Result<(), String> {
    security_framework::passwords::set_generic_password(KEYCHAIN_SERVICE, provider_id, value)
        .map_err(|_| "Could not store the API key in macOS Keychain".to_string())
}

#[cfg(not(target_os = "macos"))]
fn set_keychain_value(_provider_id: &str, _value: &[u8]) -> Result<(), String> {
    Err("API key storage requires macOS Keychain".to_string())
}

#[cfg(target_os = "macos")]
pub(crate) fn load_api_key(provider_id: &str) -> Result<Option<Zeroizing<String>>, String> {
    use security_framework::passwords::{generic_password, PasswordOptions};
    match generic_password(PasswordOptions::new_generic_password(
        KEYCHAIN_SERVICE,
        provider_id,
    )) {
        Ok(value) => match String::from_utf8(value) {
            Ok(key) => Ok(Some(Zeroizing::new(key))),
            Err(error) => {
                let mut value = error.into_bytes();
                value.zeroize();
                Err("The provider API key stored in Keychain is invalid".to_string())
            }
        },
        Err(error) if error.code() == KEYCHAIN_NOT_FOUND => Ok(None),
        Err(_) => Err("Could not read the provider API key from macOS Keychain".to_string()),
    }
}

#[cfg(not(target_os = "macos"))]
pub(crate) fn load_api_key(_provider_id: &str) -> Result<Option<Zeroizing<String>>, String> {
    Err("API key storage requires macOS Keychain".to_string())
}

#[cfg(target_os = "macos")]
fn delete_keychain_value(provider_id: &str) -> Result<(), String> {
    match security_framework::passwords::delete_generic_password(KEYCHAIN_SERVICE, provider_id) {
        Ok(()) => Ok(()),
        Err(error) if error.code() == KEYCHAIN_NOT_FOUND => Ok(()),
        Err(_) => Err("Could not delete the provider API key from macOS Keychain".to_string()),
    }
}

#[cfg(not(target_os = "macos"))]
fn delete_keychain_value(_provider_id: &str) -> Result<(), String> {
    Err("API key storage requires macOS Keychain".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn api_key_validation_rejects_empty_whitespace_control_and_oversize_values() {
        assert!(validate_api_key("").is_err());
        assert!(validate_api_key(" secret").is_err());
        assert!(validate_api_key("secret\nheader").is_err());
        assert!(validate_api_key(&"x".repeat(4_097)).is_err());
        assert!(validate_api_key("sk-valid_123").is_ok());
    }

    #[test]
    fn only_a_saved_api_key_provider_accepts_a_credential() {
        let mut settings = crate::ModelProvidersSettings {
            harness: crate::HarnessSettings {
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
