//! Bounded setup and cancellation-aware execution of the immediate registration tool.
use super::*;
/// A single budget covers connection, reconciliation and extraction.
pub(super) async fn within_budget<T>(
    operation: impl std::future::Future<Output = Result<T, String>>,
    until: tokio::time::Instant,
    parent: &RunCancellation,
    cancel: &RunCancellation,
) -> Result<T, String> {
    tokio::select! { biased;
        _=parent.cancelled()=>{cancel.cancel();Err("personal-foreground-abort".into())},
        _=cancel.cancelled()=>Err("personal-foreground-abort".into()),
        _=tokio::time::sleep_until(until)=>{cancel.cancel();Err("personal-extraction-timeout".into())},
        value=operation=>value,
    }
}
pub async fn execute(
    state: Option<&crate::AppState>,
    input: &StartTurnInput,
    arguments: &str,
    deadline: std::time::Duration,
    parent: &RunCancellation,
) -> String {
    let until = tokio::time::Instant::now() + deadline;
    let result = async {
        let args: Value =
            serde_json::from_str(arguments).map_err(|_| "world-registration-arguments")?;
        if args.as_object().is_none_or(|a| !a.is_empty()) {
            return Err("world-registration-arguments".into());
        }
        if parent.is_cancelled() {
            return Err("personal-foreground-abort".into());
        }
        let state = state.ok_or("world-current-turn-unavailable")?;
        state
            .sqlite_writer
            .read_serialized(|c| source_id(c, input))?;
        if !crate::memory::control_plane::memory_enabled() {
            return Err("world-memory-disabled".into());
        }
        let cancel = Arc::new(RunCancellation::default());
        let _foreground = within_budget(
            async { Ok(crate::memory::personal_state::scheduler::foreground().await) },
            until,
            parent,
            &cancel,
        )
        .await?;
        {
            let mut active = ACTIVE.lock().map_err(|_| "world-registration-busy")?;
            if active.is_some() {
                return Err("world-registration-busy".into());
            }
            *active = Some(cancel.clone());
        }
        let _active = Active;
        crate::memory::personal_state::local_binding::LocalBinding::load()?;
        let adapter = within_budget(
            crate::memory::personal_state::managed::Adapter::configured(
                state.sqlite_writer.clone(),
            ),
            until.min(tokio::time::Instant::now() + std::time::Duration::from_secs(10)),
            parent,
            &cancel,
        )
        .await?;
        if !adapter.product.as_ref().is_some_and(|p| p.can_generate) {
            return Err("personal-local-binding-unverified".into());
        }
        within_budget(adapter.cleanup(), until, parent, &cancel).await?;
        if !crate::memory::control_plane::memory_enabled() {
            return Err("world-memory-disabled".into());
        }
        if cancel.is_cancelled() || parent.is_cancelled() {
            return Err("personal-foreground-abort".into());
        }
        let work = extract_now(&state.sqlite_writer, &adapter, input, cancel.clone());
        tokio::pin!(work);
        // The owned extractor settles cancellation before this foreground reservation
        // is released; its existing remote-stop protocol may leave a pending receipt.
        let result = tokio::select! { biased;
            _=parent.cancelled()=>{cancel.cancel();work.await},
            _=cancel.cancelled()=>work.await,
            _=tokio::time::sleep_until(until)=>{cancel.cancel();work.await},
            result=&mut work=>result,
        };
        let cleanup = within_budget(adapter.cleanup(), until, parent, &cancel).await;
        let mut value = result?;
        if cleanup.is_err() {
            value["cleanupPending"] = json!(true);
        }
        Ok::<Value, String>(value)
    }
    .await;
    match result {
        Ok(value)=>value.to_string(),
        Err(error)=>json!({"status":"not_confirmed","reason":match error.as_str() {
            "world-memory-disabled"=>"memory-disabled",
            "world-registration-busy"=>"busy",
            "world-registration-held"=>"held",
            "world-explicit-request-required"=>"explicit-request-required",
            "world-current-turn-unavailable"=>"current-turn-unavailable",
            "world-registration-arguments"=>"invalid-arguments",
            "personal-local-binding-unverified"|"personal-local-binding-mismatch"=>"local-binding-unverified",
            _=>error_code(&error,false)
        }}).to_string()
    }
}
