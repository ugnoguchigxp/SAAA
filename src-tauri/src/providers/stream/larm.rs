use std::sync::Arc;
use std::time::Duration;

use super::super::session_store::{mark_larm_release_pending, persist_larm_selection};
use super::attempt::*;
use super::dispatch::available_agent_tools;
use crate::ipc_contract::{ConversationMessage, RuntimeEvent};
use crate::runtime::event_hub::RuntimeEventSender;
use crate::{AppState, LarmProviderSettings, RunCancellation, StartTurnInput};

pub(crate) struct LarmStreamContext<'a> {
    pub(crate) state: &'a AppState,
    pub(crate) session_id: &'a str,
    pub(crate) input: &'a StartTurnInput,
    pub(crate) on_event: &'a dyn RuntimeEventSender,
}

pub(crate) async fn stream_larm_provider(
    provider: &LarmProviderSettings,
    history: &[ConversationMessage],
    reasoning_effort: &str,
    max_output_tokens: u32,
    timeout_ms: u64,
    cancellation: Arc<RunCancellation>,
    context: LarmStreamContext<'_>,
) -> ProviderAttemptOutcome {
    use crate::providers::larm::{client::Cancellation, AllocationCleanup, LarmProvider};

    let larm = match LarmProvider::for_attempt(
        &context.state.larm_gate,
        &provider.base_url,
        provider.allocation_ttl_seconds,
        provider.allocation_startup_timeout_seconds,
    ) {
        Ok(larm) => larm,
        Err(kind) => {
            let kind = provider_failure_from_larm(kind);
            return ProviderAttemptOutcome::Failed {
                kind,
                public_message: kind.public_message(),
                output_started: false,
                cleanup: CleanupOutcome::NotStarted,
            };
        }
    };
    let cancellation_signal = Cancellation {
        flag: &cancellation.cancelled,
        notify: &cancellation.notify,
    };
    let allocation = match larm.allocate_ready(cancellation_signal).await {
        Ok(allocation) => allocation,
        Err(failure) => {
            let cleanup = match failure.cleanup {
                AllocationCleanup::NotStarted => CleanupOutcome::NotStarted,
                AllocationCleanup::Released => CleanupOutcome::Released,
                AllocationCleanup::DeferredToTtl(kind) => CleanupOutcome::DeferredToTtl { kind },
            };
            let kind = provider_failure_from_larm(failure.kind);
            if kind == ProviderFailureKind::Cancelled {
                return ProviderAttemptOutcome::Cancelled {
                    output_started: false,
                    cleanup,
                };
            }
            return ProviderAttemptOutcome::Failed {
                kind,
                public_message: kind.public_message(),
                output_started: false,
                cleanup,
            };
        }
    };

    if persist_larm_selection(context.state, context.session_id, &allocation).is_err() {
        let cleanup = cleanup_from_larm(larm.release(&allocation.allocation_id).await);
        return ProviderAttemptOutcome::Failed {
            kind: ProviderFailureKind::Internal,
            public_message: ProviderFailureKind::Internal.public_message(),
            output_started: false,
            cleanup,
        };
    }
    let selection_reason_code = match allocation.selection_reason {
        crate::providers::larm::contracts::SelectionReason::Primary => "primary",
        crate::providers::larm::contracts::SelectionReason::Other => "other",
    };
    if context
        .on_event
        .send(RuntimeEvent::ProviderSelected {
            run_id: context.input.run_id.clone(),
            provider_id: provider.id.clone(),
            provider_kind: "larm".to_string(),
            route_id: "llm-default".to_string(),
            runtime_id: allocation.selected_runtime_id.as_str().to_string(),
            fallback_used: allocation.fallback_used,
            selection_reason_code: selection_reason_code.to_string(),
        })
        .is_err()
    {
        let _ = mark_larm_release_pending(context.state, context.session_id);
        let cleanup = cleanup_from_larm(larm.release(&allocation.allocation_id).await);
        return ProviderAttemptOutcome::Failed {
            kind: ProviderFailureKind::ClientDisconnected,
            public_message: ProviderFailureKind::ClientDisconnected.public_message(),
            output_started: false,
            cleanup,
        };
    }

    let messages = history
        .iter()
        .filter_map(|message| {
            let role = match message.role.as_str() {
                "system" => "system",
                "assistant" => "assistant",
                "user" | "transcript" => "user",
                _ => return None,
            };
            Some(serde_json::json!({ "role": role, "content": message.content }))
        })
        .collect::<Vec<_>>();
    let persistence = Some(ProviderOutputPersistence {
        state: context.state,
        session_id: context.session_id,
    });
    let tools = available_agent_tools(persistence, context.input, 0, 0);
    let stream_url = match larm.websocket_stream_url() {
        Ok(url) => url,
        Err(kind) => {
            let cleanup = cleanup_from_larm(larm.release(&allocation.allocation_id).await);
            let kind = provider_failure_from_larm(kind);
            return ProviderAttemptOutcome::Failed {
                kind,
                public_message: kind.public_message(),
                output_started: false,
                cleanup,
            };
        }
    };
    let authorization = match larm.websocket_authorization() {
        Ok(value) => value,
        Err(kind) => {
            let cleanup = cleanup_from_larm(larm.release(&allocation.allocation_id).await);
            let kind = provider_failure_from_larm(kind);
            return ProviderAttemptOutcome::Failed {
                kind,
                public_message: kind.public_message(),
                output_started: false,
                cleanup,
            };
        }
    };
    let chat = crate::providers::llm_websocket::client::run(
        crate::providers::llm_websocket::client::WebSocketRunContext {
            stream_url: stream_url.as_str(),
            authorization: Some(authorization),
            allocation_id: Some(allocation.allocation_id.as_str()),
            model: "local",
            messages: &messages,
            tools: &tools,
            reasoning_effort,
            max_output_tokens,
            tool_timeout: Duration::from_secs(60),
            timeout: Duration::from_millis(timeout_ms),
            input: context.input,
            on_event: context.on_event,
            cancellation,
            output_persistence: persistence,
        },
    )
    .await;

    let persistence_failed = mark_larm_release_pending(context.state, context.session_id).is_err();
    let cleanup = cleanup_from_larm(larm.release(&allocation.allocation_id).await);
    if persistence_failed {
        return ProviderAttemptOutcome::Failed {
            kind: ProviderFailureKind::Internal,
            public_message: ProviderFailureKind::Internal.public_message(),
            output_started: matches!(
                &chat,
                Ok(crate::providers::llm_websocket::client::WebSocketRunResult::Completed(content)
                    | crate::providers::llm_websocket::client::WebSocketRunResult::Length(content))
                    if !content.is_empty()
            ),
            cleanup,
        };
    }

    match chat {
        Ok(crate::providers::llm_websocket::client::WebSocketRunResult::Completed(content)) => {
            ProviderAttemptOutcome::Completed { content, cleanup }
        }
        Ok(crate::providers::llm_websocket::client::WebSocketRunResult::Length(_)) => {
            ProviderAttemptOutcome::Failed {
                kind: ProviderFailureKind::PartialOutput,
                public_message: ProviderFailureKind::PartialOutput.public_message(),
                output_started: true,
                cleanup,
            }
        }
        Ok(crate::providers::llm_websocket::client::WebSocketRunResult::Failed {
            code,
            output_started,
        }) => {
            let kind = super::websocket_failure_kind(&code);
            ProviderAttemptOutcome::Failed {
                kind,
                public_message: kind.public_message(),
                output_started,
                cleanup,
            }
        }
        Ok(crate::providers::llm_websocket::client::WebSocketRunResult::Cancelled {
            output_started,
        }) => ProviderAttemptOutcome::Cancelled {
            output_started,
            cleanup,
        },
        Err(error) => {
            let mapped = super::map_websocket_error(error);
            match mapped {
                ProviderAttemptError::Cancelled { output_started } => {
                    ProviderAttemptOutcome::Cancelled {
                        output_started,
                        cleanup,
                    }
                }
                ProviderAttemptError::Failed {
                    kind,
                    output_started,
                } => ProviderAttemptOutcome::Failed {
                    kind,
                    public_message: kind.public_message(),
                    output_started,
                    cleanup,
                },
            }
        }
    }
}

pub(crate) fn cleanup_from_larm(
    cleanup: crate::providers::larm::client::CleanupResult,
) -> CleanupOutcome {
    match cleanup {
        crate::providers::larm::client::CleanupResult::Released => CleanupOutcome::Released,
        crate::providers::larm::client::CleanupResult::DeferredToTtl(kind) => {
            CleanupOutcome::DeferredToTtl { kind }
        }
    }
}
