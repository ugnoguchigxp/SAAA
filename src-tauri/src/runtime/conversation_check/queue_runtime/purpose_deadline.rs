//! One deadline across model, tools, and result adoption.
use super::*;
pub(crate) async fn process_conversation_answer<R: Runtime>(
    app: &tauri::AppHandle<R>,
    job: &Job,
    cancellation: Arc<RunCancellation>,
    audio_started: Arc<AtomicBool>,
    retry_blocked: Arc<AtomicBool>,
) -> Result<ConversationAnswer, String> {
    let state = app.state::<AppState>();
    let (providers, legacy_timeout, transport, fallbacks) = direct_route::prepare_transport(&state)
        .inspect_err(|_| {
            retry_blocked.store(true, Ordering::Release);
            cancellation.cancel();
        })?;
    let total_ms = match &transport {
        direct_route::ConversationTransport::Direct(r)
        | direct_route::ConversationTransport::Larm(Some(r)) => r.timeout_ms,
        direct_route::ConversationTransport::Larm(None) => legacy_timeout,
    };
    let deadline =
        tokio::time::Instant::now() + std::time::Duration::from_millis(total_ms.min(3_600_000));
    let execution = process_fixed(
        app,
        job,
        cancellation.clone(),
        audio_started,
        retry_blocked.clone(),
        providers,
        legacy_timeout,
        transport,
        deadline,
        fallbacks,
    );
    tokio::select! {
        result = execution => result,
        _ = tokio::time::sleep_until(deadline) => {
            retry_blocked.store(true, Ordering::Release);
            cancellation.cancel();
            Err("この依頼の全体期限を超えたため処理を停止しました。".into())
        }
    }
}
