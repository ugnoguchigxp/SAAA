fn developer_instructions(host_context: &str) -> String {
    if host_context.trim().is_empty() {
        return CODEX_READ_ONLY_SYSTEM_CONTEXT.to_string();
    }
    format!("{CODEX_READ_ONLY_SYSTEM_CONTEXT}\n\n{host_context}")
}
#[cfg(test)]
#[allow(clippy::too_many_arguments)]
pub(crate) fn run_codex_turn_process(
    run_id: &str,
    prompt: &str,
    workspace: &std::path::Path,
    model: &str,
    existing_thread_id: Option<&str>,
    timeout_ms: u64,
    on_event: &dyn RuntimeEventSender,
    cancellation: &RunCancellation,
) -> Result<CodexTurnOutcome, CodexTurnFailure> {
    let policy = crate::runtime::contracts::RunSupervisionPolicy::for_route(timeout_ms).map_err(
        |message| CodexTurnFailure {
            thread_id: existing_thread_id.map(str::to_string),
            message,
            code: crate::runtime::contracts::RunFailureCode::ConfigurationError,
            last_progress_at: None,
        },
    )?;
    run_codex_turn_process_with_policy_and_context(
        run_id,
        prompt,
        workspace,
        model,
        existing_thread_id,
        "",
        policy,
        on_event,
        cancellation,
    )
}
#[cfg(test)]
#[allow(clippy::too_many_arguments)]
pub(crate) fn run_codex_turn_process_with_policy(
    run_id: &str,
    prompt: &str,
    workspace: &std::path::Path,
    model: &str,
    existing_thread_id: Option<&str>,
    policy: crate::runtime::contracts::RunSupervisionPolicy,
    on_event: &dyn RuntimeEventSender,
    cancellation: &RunCancellation,
) -> Result<CodexTurnOutcome, CodexTurnFailure> {
    run_codex_turn_process_with_policy_and_context(
        run_id,
        prompt,
        workspace,
        model,
        existing_thread_id,
        "",
        policy,
        on_event,
        cancellation,
    )
}
#[cfg(test)]
#[allow(clippy::too_many_arguments)]
pub(crate) fn run_codex_turn_process_with_policy_and_context(
    run_id: &str,
    prompt: &str,
    workspace: &std::path::Path,
    model: &str,
    existing_thread_id: Option<&str>,
    host_context: &str,
    policy: crate::runtime::contracts::RunSupervisionPolicy,
    on_event: &dyn RuntimeEventSender,
    cancellation: &RunCancellation,
) -> Result<CodexTurnOutcome, CodexTurnFailure> {
    run_codex_turn_process_with_dispatch(
        run_id,
        prompt,
        workspace,
        model,
        existing_thread_id,
        host_context,
        policy,
        on_event,
        cancellation,
        None,
        false,
    )
}
