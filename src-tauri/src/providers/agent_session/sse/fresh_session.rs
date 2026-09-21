//! Follow-up sessions own cleanup even if creation finishes after cancellation.
use super::*;
pub(super) struct FreshSession {
    pub session: SessionResponse,
    pub events: Url,
    client: Client,
    provider: AgentSessionProviderSettings,
    key: Option<zeroize::Zeroizing<String>>,
    released: bool,
}
struct CancelCreation(std::sync::Arc<crate::RunCancellation>);
impl Drop for CancelCreation {
    fn drop(&mut self) {
        self.0.cancel();
    }
}
impl FreshSession {
    pub async fn create(
        client: &Client,
        provider: &AgentSessionProviderSettings,
        key: Option<&str>,
        cancellation: std::sync::Arc<crate::RunCancellation>,
        deadline: TokioInstant,
    ) -> Result<Self, ProviderFailureKind> {
        let child = std::sync::Arc::new(crate::RunCancellation::default());
        let _guard = CancelCreation(child.clone());
        let client = client.clone();
        let provider = provider.clone();
        let key = key.map(|key| zeroize::Zeroizing::new(key.to_string()));
        let mut task = tokio::spawn(async move {
            let session = super::super::creation::create_owned(
                client.clone(),
                provider.clone(),
                key.clone(),
                child,
            )
            .await?;
            let events = match super::super::session_transport(&provider, &session) {
                Ok(super::super::SessionTransport::ServerSentEvents(events)) => events,
                Err(kind) => {
                    let _ = super::super::release_session_with_retry(
                        &client,
                        &provider,
                        &session.id,
                        key.as_deref().map(String::as_str),
                    )
                    .await;
                    return Err(kind);
                }
            };
            Ok(Self {
                session,
                events,
                client,
                provider,
                key,
                released: false,
            })
        });
        tokio::select! {biased;
            _=cancellation.cancelled()=>Err(ProviderFailureKind::Cancelled),
            _=tokio::time::sleep_until(deadline)=>Err(ProviderFailureKind::Timeout),
            result=&mut task=>result.unwrap_or(Err(ProviderFailureKind::Internal)),
        }
    }
    pub async fn release(mut self) -> Result<(), ProviderFailureKind> {
        let result = super::super::release_session_with_retry(
            &self.client,
            &self.provider,
            &self.session.id,
            self.key.as_deref().map(String::as_str),
        )
        .await;
        // Normal release reports failure; Drop is reserved for interrupted owners.
        self.released = true;
        result
    }
}
impl Drop for FreshSession {
    fn drop(&mut self) {
        if self.released {
            return;
        }
        let client = self.client.clone();
        let provider = self.provider.clone();
        let id = self.session.id.clone();
        let key = self.key.clone();
        tokio::spawn(async move {
            let _ = super::super::release_session_with_retry(
                &client,
                &provider,
                &id,
                key.as_deref().map(String::as_str),
            )
            .await;
        });
    }
}
