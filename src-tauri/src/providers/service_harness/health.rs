use std::time::Duration;

use super::ServiceDescriptor;

pub(super) fn client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(10))
        .redirect(reqwest::redirect::Policy::none())
        .no_proxy()
        .build()
        .map_err(|_| "Could not initialize the Harness client".to_string())
}

pub(super) async fn probe(service: &ServiceDescriptor) -> Result<(), String> {
    let response = client()?
        .get(&service.health_url)
        .send()
        .await
        .map_err(|_| {
            crate::providers::stream::ProviderFailureKind::Network
                .public_message()
                .as_str()
                .to_string()
        })?;

    if !response.status().is_success() {
        return Err(format!(
            "Provider Harness {} health check returned HTTP {}",
            service.capability,
            response.status()
        ));
    }
    Ok(())
}
