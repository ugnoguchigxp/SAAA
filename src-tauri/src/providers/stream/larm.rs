use std::sync::Arc;
use std::time::Duration;

use serde_json::Value;

use super::super::session_store::{
    mark_larm_release_pending, mark_provider_output_started, persist_larm_request_id,
    persist_larm_selection,
};
use super::attempt::*;
use super::dispatch::{available_agent_tools, execute_agent_tool, tool_was_offered};
use crate::ipc_contract::{ConversationMessage, RuntimeEvent};
use crate::{AppState, LarmProviderSettings, RunCancellation, StartTurnInput};

pub(crate) struct LarmStreamContext<'a> {
    pub(crate) state: &'a AppState,
    pub(crate) session_id: &'a str,
    pub(crate) input: &'a StartTurnInput,
    pub(crate) on_event: &'a tauri::ipc::Channel<RuntimeEvent>,
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
    use crate::providers::larm::{
        client::{Cancellation, ChatMessage},
        AllocationCleanup, LarmProvider,
    };

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
    let mut allocation = match larm.allocate_ready(cancellation_signal).await {
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
            Some(ChatMessage {
                role,
                content: message.content.clone(),
            })
        })
        .collect::<Vec<_>>();
    let mut tool_exchanges = Vec::<Value>::new();
    let mut tool_calls_this_attempt = 0_usize;
    let mut latest_request_id = None;
    let chat_deadline = tokio::time::Instant::now() + Duration::from_millis(timeout_ms);
    let chat = loop {
        let Some(round_timeout) = chat_deadline
            .checked_duration_since(tokio::time::Instant::now())
            .filter(|remaining| !remaining.is_zero())
        else {
            break Err(crate::providers::larm::client::LarmError::new(
                crate::providers::larm::contracts::SessionFailureKind::Timeout,
                false,
            ));
        };
        let persistence = Some(ProviderOutputPersistence {
            state: context.state,
            session_id: context.session_id,
        });
        let tools = available_agent_tools(persistence, context.input, tool_calls_this_attempt);
        let round = larm
            .chat_with_tools(
                &mut allocation,
                &messages,
                &tool_exchanges,
                &tools,
                reasoning_effort,
                max_output_tokens,
                round_timeout,
                cancellation_signal,
                |delta, first| {
                    if first
                        && mark_provider_output_started(context.state, context.session_id).is_err()
                    {
                        return Err(crate::providers::larm::contracts::SessionFailureKind::Internal);
                    }
                    context
                        .on_event
                        .send(RuntimeEvent::Delta {
                            run_id: context.input.run_id.clone(),
                            text: delta.to_string(),
                        })
                        .map_err(|_| {
                            crate::providers::larm::contracts::SessionFailureKind::ClientDisconnected
                        })
                },
            )
            .await;
        match round {
            Ok(mut completion) => {
                if completion.request_id.is_some() {
                    latest_request_id = completion.request_id.clone();
                } else {
                    completion.request_id = latest_request_id.clone();
                }
                let Some(call) = completion.tool_call.clone() else {
                    break Ok(completion);
                };
                if !tool_was_offered(&tools, &call.name) {
                    break Err(crate::providers::larm::client::LarmError::new(
                        crate::providers::larm::contracts::SessionFailureKind::Protocol,
                        false,
                    ));
                }
                tool_calls_this_attempt += 1;
                let Some(tool_timeout) =
                    chat_deadline.checked_duration_since(tokio::time::Instant::now())
                else {
                    break Err(crate::providers::larm::client::LarmError::new(
                        crate::providers::larm::contracts::SessionFailureKind::Timeout,
                        false,
                    ));
                };
                let content = tokio::select! {
                    _ = cancellation.cancelled() => {
                        break Err(crate::providers::larm::client::LarmError::new(
                            crate::providers::larm::contracts::SessionFailureKind::Cancelled,
                            false,
                        ));
                    }
                    content = execute_agent_tool(persistence, context.input, &call, tool_timeout) => content,
                };
                crate::runtime::agent_tools::append_tool_exchange(
                    &mut tool_exchanges,
                    &call,
                    content,
                );
            }
            Err(error) => break Err(error),
        }
    };

    let persistence_failed = match &chat {
        Ok(completion) => {
            let request_persistence_failed = persist_larm_request_id(
                context.state,
                context.session_id,
                completion.request_id.as_ref(),
            )
            .is_err();
            let release_persistence_failed =
                mark_larm_release_pending(context.state, context.session_id).is_err();
            request_persistence_failed || release_persistence_failed
        }
        Err(_) => mark_larm_release_pending(context.state, context.session_id).is_err(),
    };
    let cleanup = cleanup_from_larm(larm.release(&allocation.allocation_id).await);
    if persistence_failed {
        return ProviderAttemptOutcome::Failed {
            kind: ProviderFailureKind::Internal,
            public_message: ProviderFailureKind::Internal.public_message(),
            output_started: chat
                .as_ref()
                .map(|completion| !completion.content.is_empty())
                .unwrap_or_else(|error| error.output_started),
            cleanup,
        };
    }

    match chat {
        Ok(completion) => ProviderAttemptOutcome::Completed {
            content: completion.content,
            cleanup,
        },
        Err(error) => {
            let kind = provider_failure_from_larm(error.kind);
            if kind == ProviderFailureKind::Cancelled {
                ProviderAttemptOutcome::Cancelled {
                    output_started: error.output_started,
                    cleanup,
                }
            } else {
                ProviderAttemptOutcome::Failed {
                    kind,
                    public_message: kind.public_message(),
                    output_started: error.output_started,
                    cleanup,
                }
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
