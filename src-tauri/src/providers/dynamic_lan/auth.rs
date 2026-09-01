use super::ProviderDescriptor;

pub(super) fn valid_provider_auth(
    descriptor: &ProviderDescriptor,
    claim_expires_at: chrono::DateTime<chrono::FixedOffset>,
) -> bool {
    let secret = descriptor
        .configuration
        .secret_fields
        .as_ref()
        .and_then(|fields| fields.api_key.as_deref())
        .filter(|value| !value.is_empty());
    let Some(credential) = descriptor.credential.as_ref() else {
        return secret.is_none();
    };
    match credential.r#type.as_str() {
        "none" => {
            credential.token.is_empty()
                && secret.is_none()
                && credential.expires_at.as_deref().is_none_or(|value| {
                    chrono::DateTime::parse_from_rfc3339(value).ok() == Some(claim_expires_at)
                })
        }
        "bearer" => {
            !credential.token.is_empty()
                && credential.token.len() <= 4_096
                && !credential
                    .token
                    .bytes()
                    .any(|byte| byte.is_ascii_whitespace())
                && secret == Some("credential.token")
                && credential.expires_at.as_deref().is_some_and(|value| {
                    chrono::DateTime::parse_from_rfc3339(value).ok() == Some(claim_expires_at)
                })
        }
        _ => false,
    }
}
