//! Operator diagnostic using production discovery, credentials, budgets and inference.
//! Does not mutate conversation history or print credentials/provider response bodies.
use crate::providers::stream::{CleanupOutcome, ModelStreamContext, ProviderAttemptOutcome};
pub use crate::role_routing::operator_configuration::enable as enable_role_routing;
use std::sync::Arc;

pub async fn check_response(host: &str) -> Result<String, String> {
    let started = std::time::Instant::now();
    let cancellation = Arc::new(crate::RunCancellation::default());
    let provider = crate::DynamicLanProviderSettings {
        id: "harness-diagnostic".into(),
        enabled: true,
        label: "Harness diagnostic".into(),
        location: "local".into(),
        host: host.into(),
        request_options: None,
    };
    let prompt = "Reply with exactly: SAAA_DYNAMIC_OK";
    let input = crate::StartTurnInput {
        run_id: format!("diagnostic_{}", uuid::Uuid::new_v4().simple()),
        conversation_id: "conversation_diagnostic".into(),
        content: prompt.into(),
        workspace_path: None,
        retry_input_message_id: None,
        source_id: None,
        scope_refs: vec![],
        input_origin: "voice".into(),
        presentation_mode: "visual".into(),
    };
    let history = [crate::ipc_contract::ConversationMessage {
        parts: None,
        id: "diagnostic_prompt".into(),
        conversation_id: input.conversation_id.clone(),
        role: "user".into(),
        content: prompt.into(),
        created_at: String::new(),
    }];
    eprintln!("stage=harness-llm-roundtrip; status=started; timeout_ms=240000");
    let outcome = crate::providers::stream::stream_dynamic_lan_provider(
        &provider,
        None,
        &history,
        240_000,
        cancellation.clone(),
        ModelStreamContext {
            reasoning_effort: "low",
            max_output_tokens: crate::providers::completion::DEFAULT_MAX_OUTPUT_TOKENS,
            input: &input,
            on_event: &DiscardDiagnosticEvents,
            cancellation,
            context_health: "green",
            context_sources: &[],
            context_omissions: &[],
            output_persistence: None,
        },
    )
    .await;
    match outcome {
        ProviderAttemptOutcome::Completed {
            content,
            cleanup: CleanupOutcome::Released,
        } if content.trim() == "SAAA_DYNAMIC_OK" => Ok(format!(
            "provider=harness-default; response=exact-match; connection=released; elapsed_ms={}",
            started.elapsed().as_millis()
        )),
        ProviderAttemptOutcome::Failed { public_message, .. } => {
            Err(public_message.as_str().into())
        }
        ProviderAttemptOutcome::Cancelled { .. } => Err("diagnostic-cancelled".into()),
        _ => Err("diagnostic-response-mismatch-or-release-incomplete".into()),
    }
}

#[derive(Clone, Copy)]
struct DiscardDiagnosticEvents;
impl crate::runtime::event_hub::RuntimeEventSender for DiscardDiagnosticEvents {
    fn send(&self, _: crate::ipc_contract::RuntimeEvent) -> tauri::Result<()> {
        Ok(())
    }
    fn clone_box(&self) -> Box<dyn crate::runtime::event_hub::RuntimeEventSender> {
        Box::new(*self)
    }
}
