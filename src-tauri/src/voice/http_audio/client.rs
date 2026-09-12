/// Claimed LAN tokens must not be sent through an environment/system HTTP proxy.
/// Normal cloud providers retain the user's configured proxy behavior.
pub(crate) fn build(
    builder: reqwest::ClientBuilder,
    claim_scoped: bool,
) -> Result<reqwest::Client, String> {
    let builder = if claim_scoped {
        builder.no_proxy()
    } else {
        builder
    };
    builder
        .build()
        .map_err(|_| "Could not initialize the audio HTTP client".into())
}
#[cfg(test)]
mod tests {
    use super::*;
    async fn server(status: axum::http::StatusCode) -> (String, tokio::task::JoinHandle<()>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let task = tokio::spawn(async move {
            axum::serve(
                listener,
                axum::Router::new().fallback(move || async move { status }),
            )
            .await
            .unwrap();
        });
        (url, task)
    }
    #[tokio::test]
    async fn claimed_audio_bypasses_proxy_but_cloud_audio_preserves_it() {
        let (upstream, upstream_task) = server(axum::http::StatusCode::NO_CONTENT).await;
        let (proxy, proxy_task) = server(axum::http::StatusCode::IM_A_TEAPOT).await;
        for (claimed, expected) in [(true, 204), (false, 418)] {
            let client = build(
                reqwest::Client::builder().proxy(reqwest::Proxy::all(&proxy).unwrap()),
                claimed,
            )
            .unwrap();
            assert_eq!(
                client
                    .get(&upstream)
                    .send()
                    .await
                    .unwrap()
                    .status()
                    .as_u16(),
                expected
            );
        }
        upstream_task.abort();
        proxy_task.abort();
    }
}
