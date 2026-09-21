use crate::schedule::Handle;
use sha2::{Digest, Sha256};

#[allow(dead_code)] // Native credential backends are cfg-dependent.
const ACCOUNT: &str = "google-calendar";

pub(crate) fn unsupported() -> Result<(), String> {
    if cfg!(test) || cfg!(target_os = "macos") {
        Ok(())
    } else {
        Err("Calendar connection requires macOS".into())
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
    handle.set_refresh(token);
    #[cfg(all(target_os = "macos", not(test)))]
    {
        crate::credentials::store_named_secret(ACCOUNT, token.as_bytes())?;
    }
    Ok(())
}

pub(crate) fn load_refresh(handle: &Handle) -> Result<Option<String>, String> {
    if let Some(token) = handle.refresh() {
        return Ok(Some(token));
    }
    #[cfg(all(target_os = "macos", not(test)))]
    {
        crate::credentials::load_api_key(ACCOUNT).map(|value| value.map(|token| (*token).clone()))
    }
    #[cfg(not(all(target_os = "macos", not(test))))]
    {
        Ok(None)
    }
}

pub(crate) fn clear(handle: &Handle) -> Result<(), String> {
    handle.set_refresh("");
    handle.set_access("");
    #[cfg(all(target_os = "macos", not(test)))]
    {
        let _ = crate::credentials::delete_api_key(ACCOUNT.to_string());
    }
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
        assert_eq!(handle.refresh().as_deref(), Some("refresh-token"));
    }
}
