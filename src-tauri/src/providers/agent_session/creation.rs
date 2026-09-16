use super::*;
use std::sync::Arc;
/// A cancelled caller leaves the creation worker responsible for the late id.
pub(super) async fn create_owned(
    client: Client,
    provider: AgentSessionProviderSettings,
    key: Option<zeroize::Zeroizing<String>>,
    cancellation: Arc<crate::RunCancellation>,
) -> Result<SessionResponse, ProviderFailureKind> {
    let task_client = client.clone();
    let task_provider = provider.clone();
    let task_key = key.clone();
    let mut task = tokio::spawn(async move {
        create_session(
            &task_client,
            &task_provider,
            task_key.as_deref().map(String::as_str),
        )
        .await
    });
    tokio::select! {biased;
        _=cancellation.cancelled()=>{
            tokio::spawn(async move {
                if let Ok(Ok(session))=task.await {
                    let _=release_session_with_retry(&client,&provider,&session.id,key.as_deref().map(String::as_str)).await;
                }
            });
            Err(ProviderFailureKind::Cancelled)
        },
        result=&mut task=>result.unwrap_or(Err(ProviderFailureKind::Internal)),
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn cancellation_releases_a_late_creation_response() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let mut provider = super::super::tests::provider();
        provider.base_url = format!("http://{}/", listener.local_addr().unwrap());
        let accepted = Arc::new(tokio::sync::Notify::new());
        let released = Arc::new(tokio::sync::Notify::new());
        let created = accepted.clone();
        let deleted = released.clone();
        let app = axum::Router::new()
            .route(
                "/v1/agents/sessions",
                axum::routing::post(move || {
                    let created = created.clone();
                    async move {
                        created.notify_one();
                        tokio::time::sleep(Duration::from_millis(30)).await;
                        axum::Json(json!({"id":"session-late","events_url":"/events"}))
                    }
                }),
            )
            .route(
                "/v1/agents/sessions/session-late/release",
                axum::routing::post(move || {
                    let deleted = deleted.clone();
                    async move {
                        deleted.notify_one();
                        axum::http::StatusCode::NO_CONTENT
                    }
                }),
            );
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let cancellation = Arc::new(crate::RunCancellation::default());
        let cancel = cancellation.clone();
        let task = tokio::spawn(async move {
            create_owned(
                provider_client(&provider, Duration::from_secs(1)).unwrap(),
                provider,
                None,
                cancel,
            )
            .await
        });
        tokio::time::timeout(Duration::from_secs(1), accepted.notified())
            .await
            .unwrap();
        cancellation.cancel();
        assert_eq!(
            task.await.unwrap().unwrap_err(),
            ProviderFailureKind::Cancelled
        );
        tokio::time::timeout(Duration::from_secs(1), released.notified())
            .await
            .unwrap();
        server.abort();
    }
}
