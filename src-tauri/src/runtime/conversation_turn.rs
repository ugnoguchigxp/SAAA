//! Conversation provider path. `turns::execute_turn` dispatches here after coding / capability.
#[path = "conversation_context.rs"]
mod conversation_context;
#[path = "conversation_controller/mod.rs"]
mod conversation_controller;
#[path = "conversation_inputs.rs"]
mod conversation_inputs;
#[path = "conversation_prepare.rs"]
mod prepare;
#[cfg(test)]
use prepare::world_free_history;
#[path = "conversation_codex_dispatch.rs"]
mod codex_dispatch;
#[path = "conversation_provider_route.rs"]
mod provider_route;
#[path = "role_step_sink.rs"]
mod role_step_sink;
#[path = "conversation_role_steps.rs"]
mod role_steps;
#[path = "conversation_state_answer.rs"]
mod state_answer;
#[path = "conversation_stream.rs"]
mod streaming;
#[cfg(test)]
use role_steps::should_share_larm_voice_session;

use super::event_hub::RuntimeEventSender;
use crate::ipc_contract::{ConversationMessage, RuntimeEvent};
use crate::{
    memory, persist_conversation_success_with_state, stream_model_provider,
    stream_voice_aware_dynamic_lan_provider, AppState, CleanupOutcome, ModelProviderSettings,
    ModelStreamContext, ProviderAttemptOutcome, ProviderFailureKind, ProviderOutputPersistence,
    RunCancellation, StartTurnInput, TurnExecutionFailure,
};
use conversation_context::compose_provider_history;
use std::sync::Arc;
#[path = "conversation_role_codex.rs"]
mod role_codex;
#[cfg(test)]
use role_codex::role_codex_prompt;

pub(crate) async fn execute_conversation_turn(
    state: &AppState,
    input: &StartTurnInput,
    on_event: &dyn RuntimeEventSender,
    cancellation: Arc<RunCancellation>,
) -> Result<ConversationMessage, TurnExecutionFailure> {
    execute_conversation_turn_with_candidates(state, input, on_event, cancellation, Vec::new())
        .await
}

#[derive(Clone)]
struct RoleCandidate {
    step_id: String,
    purpose: String,
    content: String,
}

#[derive(Clone)]
struct ActiveRoleStep {
    step_id: String,
    purpose: String,
    revision: i64,
    config_fingerprint: String,
}

async fn execute_conversation_turn_with_candidates(
    state: &AppState,
    input: &StartTurnInput,
    on_event: &dyn RuntimeEventSender,
    cancellation: Arc<RunCancellation>,
    role_candidates: Vec<RoleCandidate>,
) -> Result<ConversationMessage, TurnExecutionFailure> {
    let context_started = std::time::Instant::now();
    let conversation_inputs::Inputs {
        mut providers,
        route,
        security,
        identity,
        regional,
        scope,
        personal_source_error,
        configuration_fingerprint,
        role_dispatch,
        ..
    } = conversation_inputs::load(state, input)?;
    let role_provider_step = matches!(
        &role_dispatch,
        Some(conversation_inputs::conversation_inputs_roles::RoleDispatch::Provider { .. })
    );
    let role_provider_max_input_bytes = match &role_dispatch {
        Some(conversation_inputs::conversation_inputs_roles::RoleDispatch::Provider {
            max_input_bytes,
        }) => Some(*max_input_bytes as usize),
        _ => None,
    };
    let active_provider_step = if role_provider_step {
        state.sqlite_writer.write(|connection| {
            connection
                .query_row(
                    "SELECT id,purpose,revision,config_fingerprint FROM rr_steps WHERE root_id=?1 AND status='running' ORDER BY ordinal LIMIT 1",
                    [&input.run_id],
                    |row| {
                        Ok(ActiveRoleStep {
                            step_id: row.get(0)?,
                            purpose: row.get(1)?,
                            revision: row.get(2)?,
                            config_fingerprint: row.get(3)?,
                        })
                    },
                )
                .map(Some)
                .map_err(|error| error.to_string())
        })?
    } else {
        None
    };
    crate::providers::http_metrics::record("contextInputsLoad", context_started.elapsed());
    if scope.status != "resolved" {
        crate::runtime::context::generation::record_red(
            state,
            &input.run_id,
            scope.reason_code.as_deref().unwrap_or("scope-invalid"),
        );
        return Err(TurnExecutionFailure::configuration(format!(
            "Context scope could not be resolved: {}",
            scope.reason_code.as_deref().unwrap_or("scope-invalid")
        )));
    }
    if let Some(error) = personal_source_error {
        crate::runtime::context::generation::record_red(
            state,
            &input.run_id,
            "personal-state-source-unavailable",
        );
        return Err(TurnExecutionFailure::configuration(error));
    }
    // Narrow present-state questions have a host-verifiable answer. Persist the card through the
    // normal completion path so the UI and voice completion still share one message, but never
    // ask a provider to turn an unavailable observation into a current-state assertion.
    let state_query = crate::runtime::context::state_answer::is_state_query(&input.content);
    let verified_events = on_event;
    let claim_events =
        crate::runtime::context::world::state_claim::ClaimEvents(on_event.clone_box());
    let on_event: &dyn RuntimeEventSender = if state_query { &claim_events } else { on_event };
    if state_query && (route.source == "harness" || role_dispatch.is_some()) {
        return state_answer::persist_card(state, input, verified_events);
    }
    // Compose only at a concrete provider dispatch boundary. A generic pre-compose would use
    // the wrong provider budget and could reject a request that fits its selected provider.
    crate::providers::http_metrics::record("contextAssemblyTotal", context_started.elapsed());

    if let Some(conversation_inputs::conversation_inputs_roles::RoleDispatch::CodexSdk {
        model,
        max_input_bytes,
        binding,
    }) = role_dispatch
    {
        return codex_dispatch::execute(
            state,
            input,
            on_event,
            cancellation,
            role_candidates,
            &identity,
            &regional,
            route.timeout_ms,
            model,
            max_input_bytes,
            binding,
        )
        .await;
    }
    provider_route::execute(
        state,
        input,
        on_event,
        cancellation,
        role_candidates,
        &mut providers,
        &route,
        &security,
        &identity,
        &regional,
        &configuration_fingerprint,
        state_query,
        verified_events,
        active_provider_step.as_ref(),
        role_provider_step,
        role_provider_max_input_bytes,
    )
    .await
}

#[path = "conversation_recovery.rs"]
mod recovery;
#[cfg(test)]
pub(crate) use recovery::provider_fallback_allowed;
#[cfg(test)]
use recovery::provider_route_fallback_allowed;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn role_routed_voice_reasoning_reuses_the_lfm_session() {
        let reasoning_request_id = "lfm_reasoning_utterance-1";

        assert!(should_share_larm_voice_session(
            "provider",
            Some(crate::DYNAMIC_LAN_PROVIDER_ID),
            "voice",
            Some(reasoning_request_id),
        ));
        assert!(!should_share_larm_voice_session(
            "provider",
            Some("some-other-provider"),
            "voice",
            Some(reasoning_request_id),
        ));
        assert!(!should_share_larm_voice_session(
            "provider",
            Some(crate::DYNAMIC_LAN_PROVIDER_ID),
            "text",
            Some(reasoning_request_id),
        ));
    }

    #[test]
    fn rr_19_codex_prompt_keeps_current_request_and_bounds_history() {
        let history = vec![ConversationMessage {
            parts: None,
            id: "old".into(),
            conversation_id: "c".into(),
            role: "assistant".into(),
            content: "x".repeat(60_000),
            created_at: "1".into(),
        }];
        let prompt =
            role_codex_prompt(&history, "current request", 48 * 1024).expect("bounded prompt");
        assert!(prompt.contains("current request"));
        assert!(prompt.len() < 50_000);
        assert!(prompt.contains("untrusted conversation data"));
    }

    #[test]
    fn rr_19_codex_prompt_rejects_an_unfit_current_request() {
        let error =
            role_codex_prompt(&[], &"x".repeat(512), 128).expect_err("must not trim user request");
        assert!(error.message.contains("input limit"));
    }

    #[test]
    fn world_free_history_removes_the_world_block_for_unsupported_providers() {
        let world = crate::runtime::context::world::turn::WorldLive::for_test(
            true,
            "WITH_WORLD",
            Some("WITHOUT_WORLD"),
        );
        let history = vec![
            ConversationMessage {
                parts: None,
                id: "system".into(),
                conversation_id: "c".into(),
                role: "system".into(),
                content: "policy".into(),
                created_at: "system".into(),
            },
            ConversationMessage {
                parts: None,
                id: "world".into(),
                conversation_id: "c".into(),
                role: "assistant".into(),
                content: "WITH_WORLD".into(),
                created_at: "1".into(),
            },
            ConversationMessage {
                parts: None,
                id: "user".into(),
                conversation_id: "c".into(),
                role: "user".into(),
                content: "hello".into(),
                created_at: "2".into(),
            },
        ];
        let stripped = world_free_history(&history, Some(&world)).expect("world block present");
        assert_eq!(stripped.len(), 3);
        assert_eq!(stripped[1].content, "WITHOUT_WORLD");
        assert_eq!(stripped[2].content, "hello");
        // When the World was the only selected candidate the block disappears entirely.
        let only_world =
            crate::runtime::context::world::turn::WorldLive::for_test(true, "WITH_WORLD", None);
        let stripped =
            world_free_history(&history, Some(&only_world)).expect("world block present");
        assert_eq!(stripped.len(), 2);
        assert_eq!(stripped[0].content, "policy");
        assert_eq!(stripped[1].content, "hello");
        // Without a World there is nothing to strip, so the original borrow is reused.
        assert!(world_free_history(&history, None).is_none());
        assert!(world_free_history(
            &history,
            Some(&crate::runtime::context::world::turn::WorldLive::without_blocks())
        )
        .is_none());
    }

    #[test]
    fn response_retry_reuses_the_failed_input_message() {
        let connection = rusqlite::Connection::open_in_memory().expect("database opens");
        crate::persistence::schema::initialize_database(&connection).expect("database initializes");
        let state = crate::test_support::app_state(connection);
        let first = StartTurnInput {
            run_id: "run-first".to_string(),
            conversation_id: crate::PRIMARY_CONVERSATION_ID.to_string(),
            content: "retry this response".to_string(),
            workspace_path: None,
            retry_input_message_id: None,
            source_id: None,
            scope_refs: Vec::new(),
            input_origin: "text".to_string(),
            presentation_mode: "visual".to_string(),
        };
        crate::runtime::turns::prepare_runtime_run(&state, &first).expect("first run prepares");
        let input_message_id: String = state
            .sqlite_writer
            .lock()
            .expect("database lock")
            .query_row(
                "SELECT input_message_id FROM runtime_runs WHERE id = ?1",
                [&first.run_id],
                |row| row.get(0),
            )
            .expect("input message reads");
        state
            .sqlite_writer
            .lock()
            .expect("database lock")
            .execute(
                "UPDATE runtime_runs SET status = 'failed' WHERE id = ?1",
                [&first.run_id],
            )
            .expect("first run fails");

        let retry = StartTurnInput {
            run_id: "run-retry".to_string(),
            retry_input_message_id: Some(input_message_id.clone()),
            source_id: None,
            scope_refs: Vec::new(),
            input_origin: "text".to_string(),
            presentation_mode: "visual".to_string(),
            ..first
        };
        crate::runtime::turns::prepare_runtime_run(&state, &retry).expect("retry prepares");
        let connection = state.sqlite_writer.lock().expect("database lock");
        let message_count: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM conversation_messages WHERE role = 'user'",
                [],
                |row| row.get(0),
            )
            .expect("message count reads");
        let retry_input: String = connection
            .query_row(
                "SELECT input_message_id FROM runtime_runs WHERE id = 'run-retry'",
                [],
                |row| row.get(0),
            )
            .expect("retry input reads");
        assert_eq!(message_count, 1);
        assert_eq!(retry_input, input_message_id);
    }

    #[test]
    fn provider_fallback_policy_is_failure_kind_and_output_aware() {
        for kind in [
            ProviderFailureKind::Capacity,
            ProviderFailureKind::Unavailable,
            ProviderFailureKind::Upstream,
            ProviderFailureKind::Network,
            ProviderFailureKind::Timeout,
            ProviderFailureKind::AllocationLost,
        ] {
            assert!(provider_fallback_allowed(kind, false), "{}", kind.as_str());
            assert!(!provider_fallback_allowed(kind, true), "{}", kind.as_str());
        }
        for kind in [
            ProviderFailureKind::Policy,
            ProviderFailureKind::Authentication,
            ProviderFailureKind::Contract,
            ProviderFailureKind::Protocol,
            ProviderFailureKind::RequestTooLarge,
            ProviderFailureKind::RequiredContextOverflow,
            ProviderFailureKind::PartialOutput,
            ProviderFailureKind::ClientDisconnected,
            ProviderFailureKind::Cancelled,
            ProviderFailureKind::Internal,
        ] {
            assert!(!provider_fallback_allowed(kind, false), "{}", kind.as_str());
            assert!(!provider_fallback_allowed(kind, true), "{}", kind.as_str());
        }
    }

    #[test]
    fn authentication_never_switches_providers() {
        let dynamic_lan = crate::test_support::dynamic_lan_provider("dynamic_lan-primary");
        let direct = crate::test_support::provider("direct-primary", "local");
        assert!(!provider_route_fallback_allowed(
            &dynamic_lan,
            ProviderFailureKind::Authentication,
            false
        ));
        assert!(!provider_route_fallback_allowed(
            &dynamic_lan,
            ProviderFailureKind::Authentication,
            true
        ));
        assert!(!provider_route_fallback_allowed(
            &direct,
            ProviderFailureKind::Authentication,
            false
        ));
    }
}
