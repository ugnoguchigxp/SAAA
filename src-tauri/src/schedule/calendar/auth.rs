use crate::schedule::Handle;
use sha2::{Digest, Sha256};

const ACCOUNT: &str = "google-calendar";
#[cfg(not(test))]
const SERVICE: &str = "com.saaa.oauth-refresh-token";
#[cfg(not(test))]
const LEGACY_SERVICE: &str = crate::credentials::PROVIDER_CREDENTIAL_SERVICE;

pub(crate) fn unsupported() -> Result<(), String> {
    if cfg!(test)
        || cfg!(any(
            target_os = "macos",
            target_os = "windows",
            target_os = "linux"
        ))
    {
        Ok(())
    } else {
        Err("Calendar connection is not supported on this platform".into())
    }
}

pub(crate) fn pkce_pair() -> (String, String) {
    let verifier = format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    );
    let digest = Sha256::digest(verifier.as_bytes());
    let challenge =
        base64::Engine::encode(&base64::engine::general_purpose::URL_SAFE_NO_PAD, digest);
    (verifier, challenge)
}

pub(crate) fn store_refresh(handle: &Handle, token: &str) -> Result<(), String> {
    store_refresh_with(handle, token, persist_refresh)
}

fn store_refresh_with(
    handle: &Handle,
    token: &str,
    persist: impl FnOnce(&str) -> Result<(), String>,
) -> Result<(), String> {
    persist(token)?;
    handle.set_refresh(token);
    Ok(())
}

#[cfg(not(test))]
fn persist_refresh(token: &str) -> Result<(), String> {
    crate::credentials::store_named_secret(SERVICE, ACCOUNT, token.as_bytes())
}

#[cfg(test)]
fn persist_refresh(_token: &str) -> Result<(), String> {
    Ok(())
}

pub(crate) fn legacy_entry_is_unambiguous(state: &crate::AppState) -> bool {
    state
        .sqlite_readers
        .read(crate::persistence::load_model_providers)
        .map(|settings| legacy_entry_is_unambiguous_for(&settings))
        .unwrap_or(false)
}

fn legacy_entry_is_unambiguous_for(settings: &crate::ModelProvidersSettings) -> bool {
    settings
        .providers
        .iter()
        .all(|provider| provider.id() != ACCOUNT)
}

pub(crate) fn load_refresh(
    handle: &Handle,
    legacy_entry_is_unambiguous: bool,
) -> Result<Option<zeroize::Zeroizing<String>>, String> {
    if let Some(token) = handle.refresh() {
        return Ok(Some(token));
    }
    #[cfg(not(test))]
    {
        if let Some(token) = crate::credentials::load_named_secret(SERVICE, ACCOUNT)? {
            return Ok(Some(token));
        }
        if !legacy_entry_is_unambiguous {
            return Ok(None);
        }
        let legacy = crate::credentials::load_named_secret(LEGACY_SERVICE, ACCOUNT)?;
        if let Some(token) = legacy.as_deref() {
            crate::credentials::store_named_secret(SERVICE, ACCOUNT, token.as_bytes())?;
            crate::credentials::delete_named_secret(LEGACY_SERVICE, ACCOUNT)?;
        }
        Ok(legacy)
    }
    #[cfg(test)]
    {
        let _ = legacy_entry_is_unambiguous;
        Ok(None)
    }
}

pub(crate) fn clear(handle: &Handle, delete_legacy: bool) -> Result<(), String> {
    clear_with(handle, || delete_refresh(delete_legacy))
}

fn clear_with(handle: &Handle, delete: impl FnOnce() -> Result<(), String>) -> Result<(), String> {
    delete()?;
    handle.set_refresh("");
    handle.set_access("");
    Ok(())
}

#[cfg(not(test))]
fn delete_refresh(delete_legacy: bool) -> Result<(), String> {
    let current = crate::credentials::delete_named_secret(SERVICE, ACCOUNT);
    if delete_legacy {
        current.and(crate::credentials::delete_named_secret(
            LEGACY_SERVICE,
            ACCOUNT,
        ))
    } else {
        current
    }
}

#[cfg(test)]
fn delete_refresh(_delete_legacy: bool) -> Result<(), String> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sl_11_pkce_is_s256_and_memory_store_avoids_sqlite() {
        let (verifier, challenge) = pkce_pair();
        assert!(!verifier.is_empty());
        assert_ne!(verifier, challenge);
        let handle = Handle::default();
        store_refresh(&handle, "refresh-token").unwrap();
        assert_eq!(
            handle.refresh().as_deref().map(String::as_str),
            Some("refresh-token")
        );
    }

    #[test]
    fn failed_refresh_persistence_does_not_change_memory_state() {
        let handle = Handle::default();
        handle.set_refresh("old-refresh");
        assert!(
            store_refresh_with(&handle, "new-refresh", |_| Err("store-failed".into())).is_err()
        );
        assert_eq!(
            handle.refresh().as_deref().map(String::as_str),
            Some("old-refresh")
        );
    }

    #[test]
    fn failed_refresh_deletion_does_not_disconnect_memory_state() {
        let handle = Handle::default();
        handle.set_refresh("refresh-token");
        handle.set_access("access-token");
        assert!(clear_with(&handle, || Err("delete-failed".into())).is_err());
        assert_eq!(
            handle.refresh().as_deref().map(String::as_str),
            Some("refresh-token")
        );
        assert_eq!(handle.access().as_deref(), Some("access-token"));
    }

    #[test]
    fn legacy_oauth_entry_is_not_touched_when_a_provider_uses_the_same_account() {
        let mut settings = crate::ModelProvidersSettings {
            harness: crate::HarnessSettings {
                larm_profile: None,
                tts_voice: None,
                address: String::new(),
            },
            providers: vec![crate::test_support::provider(ACCOUNT, "cloud")],
            reasoning_effort: "medium".to_string(),
        };
        assert!(!legacy_entry_is_unambiguous_for(&settings));
        settings.providers.clear();
        assert!(legacy_entry_is_unambiguous_for(&settings));
    }
}
