use super::conversation_controller;
use super::conversation_controller::execute as execute_reasoning;
use super::prepare::{
    compose_after_connect, provider_input_budget, world_free_history, FreshProviderContext,
};
use super::recovery::{
    context_recovery_message, provider_fallback_allowed, provider_route_fallback_allowed,
};
use super::role_steps::{
    await_premium_step, execute_specialist_request, parse_review_response, role_step_request,
    should_share_larm_voice_session,
};
use super::{role_step_sink, state_answer, streaming, ActiveRoleStep, RoleCandidate};
use crate::ipc_contract::{ConversationMessage, RuntimeEvent};
use crate::providers::routing::{effective_conversation_route_ids, resolve_harness_llm_provider};
use crate::runtime::event_hub::RuntimeEventSender;
use crate::{
    begin_provider_session, finish_dynamic_lan_provider_session, finish_provider_session, memory,
    now_iso, persist_conversation_success_with_state, update_runtime_provider, AppState,
    ModelProviderSettings, ModelStreamContext, ProviderAttemptOutcome, ProviderFailureKind,
    ProviderOutputPersistence, RunCancellation, StartTurnInput, TurnExecutionFailure,
};
use std::sync::Arc;
include!("conversation_provider_route.d/01.rs");
include!("conversation_provider_route.d/02.rs");
