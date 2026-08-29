use serde_json::{json, Value};
use std::io::Write;
use std::sync::mpsc;
use std::time::Duration;

use crate::process_guard::ProcessGuard;
use crate::redact::{bounded_text, redact_runtime_text};
use crate::{
    write_codex_message, CodexReaderMessage, CodexTurnFailure, CodexTurnOutcome, RunCancellation,
};

pub(crate) fn elapsed_millis(started: std::time::Instant) -> u64 {
    u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX)
}

pub(crate) fn supervisor_wait_duration(
    supervisor: &crate::runtime::supervisor::RunSupervisor,
    now_ms: u64,
) -> Duration {
    let remaining_ms = supervisor
        .next_deadline_ms()
        .map(|deadline| deadline.saturating_sub(now_ms))
        .unwrap_or(100);
    Duration::from_millis(remaining_ms.clamp(1, 100))
}

pub(crate) fn apply_supervisor_actions(
    actions: &[crate::runtime::contracts::SupervisorAction],
    child: &mut ProcessGuard,
    stdin: &mut impl Write,
    thread_id: &str,
    turn_id: &str,
    pending_outcome: Option<crate::runtime::contracts::RunOutcome>,
) -> Option<crate::runtime::contracts::RunOutcome> {
    use crate::runtime::contracts::SupervisorAction;
    let mut outcome = None;
    for action in actions {
        match action {
            SupervisorAction::SendInterrupt => {
                if write_codex_message(
                    stdin,
                    json!({
                        "method": "turn/interrupt",
                        "id": 4,
                        "params": { "threadId": thread_id, "turnId": turn_id }
                    }),
                )
                .is_err()
                {
                    outcome =
                        pending_outcome.or(Some(crate::runtime::contracts::RunOutcome::Failed(
                            crate::runtime::contracts::RunFailureCode::InternalError,
                        )));
                }
            }
            SupervisorAction::ForceKill => {
                child.terminate();
            }
            SupervisorAction::Finish(value) => outcome = Some(*value),
        }
    }
    outcome
}

pub(crate) fn supervisor_outcome(
    outcome: crate::runtime::contracts::RunOutcome,
    thread_id: &str,
    content: &str,
    failure_detail: Option<String>,
    last_progress_at: Option<String>,
) -> Result<CodexTurnOutcome, CodexTurnFailure> {
    use crate::runtime::contracts::{RunFailureCode, RunOutcome};
    match outcome {
        RunOutcome::Completed => Ok(CodexTurnOutcome {
            thread_id: thread_id.to_string(),
            content: bounded_text(content, 64_000),
            last_progress_at,
        }),
        RunOutcome::Cancelled => Err(CodexTurnFailure {
            thread_id: Some(thread_id.to_string()),
            message: "Codex turn cancelled by user".to_string(),
            code: RunFailureCode::UserCancelled,
            last_progress_at,
        }),
        RunOutcome::Failed(code) => Err(CodexTurnFailure {
            thread_id: Some(thread_id.to_string()),
            message: redact_runtime_text(&failure_detail.unwrap_or_else(|| {
                match code {
                    RunFailureCode::RequestTimeout => "Codex request timed out",
                    RunFailureCode::ProgressTimeout => "Codex progress stopped",
                    RunFailureCode::TerminalTimeout => "Codex terminal event was not received",
                    RunFailureCode::HardTimeout => "Codex route reached its hard timeout",
                    RunFailureCode::ChildExited => "Codex app-server exited unexpectedly",
                    RunFailureCode::ProtocolError => "Codex app-server protocol error",
                    RunFailureCode::PolicyViolation => "Codex read-only policy violation",
                    RunFailureCode::ProviderError => "Codex turn failed",
                    RunFailureCode::ConfigurationError => "Codex configuration is invalid",
                    RunFailureCode::ChildStartFailed => "Codex app-server could not start",
                    RunFailureCode::ResponseTooLarge => "Codex response was too large",
                    RunFailureCode::InternalError => "Codex runtime internal error",
                    RunFailureCode::UserCancelled => "Codex turn cancelled by user",
                    RunFailureCode::AppRestarted => "Application restarted during the run",
                }
                .to_string()
            })),
            code,
            last_progress_at,
        }),
    }
}

pub(crate) fn receive_supervised_codex_result(
    receiver: &mpsc::Receiver<CodexReaderMessage>,
    request_id: u64,
    supervisor: &mut crate::runtime::supervisor::RunSupervisor,
    origin: std::time::Instant,
    cancellation: &RunCancellation,
) -> Result<Value, crate::runtime::contracts::RunFailureCode> {
    use crate::runtime::contracts::{RunFailureCode, RunOutcome, RunSignal, SupervisorAction};

    supervisor.begin_request(elapsed_millis(origin));
    loop {
        if cancellation.is_cancelled() {
            supervisor.apply(elapsed_millis(origin), RunSignal::CancelRequested);
            return Err(RunFailureCode::UserCancelled);
        }
        let now_ms = elapsed_millis(origin);
        if let Some(code) = supervisor
            .tick(now_ms)
            .into_iter()
            .find_map(|action| match action {
                SupervisorAction::Finish(RunOutcome::Failed(code)) => Some(code),
                _ => None,
            })
        {
            return Err(code);
        }
        let message = match receiver.recv_timeout(supervisor_wait_duration(supervisor, now_ms)) {
            Ok(CodexReaderMessage::Message(message)) => message,
            Ok(CodexReaderMessage::Failed { code, .. }) => return Err(code),
            Err(mpsc::RecvTimeoutError::Timeout) => {
                if cancellation.is_cancelled() {
                    supervisor.apply(elapsed_millis(origin), RunSignal::CancelRequested);
                    return Err(RunFailureCode::UserCancelled);
                }
                let now_ms = elapsed_millis(origin);
                if let Some(code) =
                    supervisor
                        .tick(now_ms)
                        .into_iter()
                        .find_map(|action| match action {
                            SupervisorAction::Finish(RunOutcome::Failed(code)) => Some(code),
                            _ => None,
                        })
                {
                    return Err(code);
                }
                continue;
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => return Err(RunFailureCode::ChildExited),
        };
        if message.get("id").is_some() && message.get("method").is_some() {
            return Err(RunFailureCode::PolicyViolation);
        }
        if message.get("id").and_then(Value::as_u64) != Some(request_id) {
            let now_ms = elapsed_millis(origin);
            if let Some(code) =
                supervisor
                    .tick(now_ms)
                    .into_iter()
                    .find_map(|action| match action {
                        SupervisorAction::Finish(RunOutcome::Failed(code)) => Some(code),
                        _ => None,
                    })
            {
                return Err(code);
            }
            continue;
        }
        supervisor.complete_request();
        if message.get("error").is_some() {
            return Err(RunFailureCode::ProviderError);
        }
        return Ok(message);
    }
}

pub(crate) fn request_failure_message(
    code: crate::runtime::contracts::RunFailureCode,
) -> &'static str {
    use crate::runtime::contracts::RunFailureCode;
    match code {
        RunFailureCode::UserCancelled => "Codex request was cancelled",
        RunFailureCode::RequestTimeout => "Codex request timed out",
        RunFailureCode::ChildExited => "Codex app-server stopped before responding",
        RunFailureCode::ProtocolError => "Codex app-server returned invalid output",
        RunFailureCode::PolicyViolation => "Codex requested a forbidden approval",
        RunFailureCode::ProviderError => "Codex app-server rejected the request",
        _ => "Codex request failed",
    }
}
