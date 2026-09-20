//! Runtime wiring for `/capability` commands (plan 12.7, C11). Parse/display stay in
//! `capability_commands`.

use std::sync::Arc;

use super::{parse, CapabilityCommand, CommandError};
use crate::generated_capabilities::generation::contracts::{GenerateInput, GenerationContext};
use crate::ipc_contract::RuntimeEvent;
use crate::runtime::event_hub::RuntimeEventSender;
use crate::{
    persist_conversation_success, AppState, RunCancellation, StartTurnInput, TurnExecutionFailure,
};

pub(crate) async fn handle_user_turn(
    state: &AppState,
    input: &StartTurnInput,
    on_event: &dyn RuntimeEventSender,
    cancellation: Arc<RunCancellation>,
) -> Result<bool, TurnExecutionFailure> {
    match parse(&input.content) {
        Err(CommandError::NotACommand) => Ok(false),
        Err(CommandError::Malformed) => {
            complete_command(
                state,
                input,
                on_event,
                "That is not a valid /capability command. Use `/capability generate <request-id>`, `/capability update <request-id> <revision-id>`, or `/capability inspect <call-id>`. Natural-language generation is not offered.",
            )
            .await?;
            Ok(true)
        }
        Ok(command) => {
            let content = dispatch(state, input, command, cancellation).await;
            complete_command(state, input, on_event, &content).await?;
            Ok(true)
        }
    }
}

async fn dispatch(
    state: &AppState,
    input: &StartTurnInput,
    command: CapabilityCommand,
    cancellation: Arc<RunCancellation>,
) -> String {
    match command {
        CapabilityCommand::Inspect { call_id } => inspect_reply(state, call_id, cancellation).await,
        CapabilityCommand::Generate { request_id } => {
            generate_reply(state, input, request_id, None, cancellation).await
        }
        CapabilityCommand::Update {
            request_id,
            base_revision_id,
        } => {
            generate_reply(
                state,
                input,
                request_id,
                Some(base_revision_id),
                cancellation,
            )
            .await
        }
    }
}

async fn inspect_reply(
    state: &AppState,
    call_id: String,
    cancellation: Arc<RunCancellation>,
) -> String {
    let Ok(principal_id) = crate::tool_selection::service::ensure_principal(&state.sqlite_writer)
    else {
        return "Capability inspection is unavailable because no principal is recorded.".into();
    };
    super::capability_inspect::inspect_reply(state, &principal_id, &call_id, &cancellation).await
}

async fn generate_reply(
    state: &AppState,
    input: &StartTurnInput,
    request_id: String,
    base_revision_id: Option<String>,
    cancellation: Arc<RunCancellation>,
) -> String {
    let Some(generation) = state.generation.clone() else {
        return "Capability generation is not configured on this host. Natural-language generation is not offered.".into();
    };
    let Ok(principal_id) = crate::tool_selection::service::ensure_principal(&state.sqlite_writer)
    else {
        return "Capability generation is unavailable because no principal is recorded.".into();
    };
    let Some(input_message_id) = state
        .sqlite_readers
        .read(|connection| {
            connection
                .query_row(
                    "SELECT input_message_id FROM runtime_runs WHERE id = ?1",
                    rusqlite::params![input.run_id],
                    |row| row.get::<_, Option<String>>(0),
                )
                .map_err(|error| error.to_string())
        })
        .ok()
        .flatten()
        .filter(|id| !id.is_empty())
    else {
        return "Capability generation is unavailable because this run has no input message."
            .into();
    };
    let receipt = generation
        .generate(
            GenerationContext {
                principal_id,
                conversation_id: input.conversation_id.clone(),
                run_id: input.run_id.clone(),
                input_message_id,
                project_id: None,
            },
            GenerateInput {
                request_id: request_id.clone(),
                base_revision_id,
            },
            (*cancellation).clone(),
        )
        .await;
    format!(
        "Capability generation\nrequest: {request_id}\nstatus: {}\njob: {}\nrevision: {}\nerror: {}\nAfter a revision is active, search and call it through the usual conversation tools. Do not expect a side-effect tool to run automatically. Natural-language generation is not offered.",
        receipt.status.as_str(),
        receipt.job_id,
        receipt.revision_id.as_deref().unwrap_or("-"),
        receipt
            .error_code
            .map(|code| code.as_str().to_string())
            .unwrap_or_else(|| "-".into()),
    )
}

async fn complete_command(
    state: &AppState,
    input: &StartTurnInput,
    on_event: &dyn RuntimeEventSender,
    content: &str,
) -> Result<(), TurnExecutionFailure> {
    let _ = on_event.send(RuntimeEvent::Started {
        run_id: input.run_id.clone(),
        route: "conversation.respond".to_string(),
        provider_id: "capability".to_string(),
    });
    let message = persist_conversation_success(state, input, content)?;
    crate::memory::personal_state::output::send_completed(state, input, on_event, &message)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ipc_contract::RuntimeEvent;

    #[derive(Clone, Default)]
    struct Sink(std::sync::Arc<std::sync::Mutex<Vec<RuntimeEvent>>>);

    impl RuntimeEventSender for Sink {
        fn send(&self, event: RuntimeEvent) -> tauri::Result<()> {
            self.0.lock().expect("events").push(event);
            Ok(())
        }
        fn clone_box(&self) -> Box<dyn RuntimeEventSender> {
            Box::new(self.clone())
        }
    }

    fn turn(run_id: &str, content: &str) -> crate::StartTurnInput {
        crate::StartTurnInput {
            run_id: run_id.into(),
            conversation_id: crate::PRIMARY_CONVERSATION_ID.into(),
            content: content.into(),
            workspace_path: None,
            retry_input_message_id: None,
            source_id: None,
            scope_refs: Vec::new(),
            input_origin: "text".into(),
            presentation_mode: "visual".into(),
        }
    }

    #[tokio::test]
    async fn rw_06_ordinary_text_is_not_intercepted() {
        let connection = rusqlite::Connection::open_in_memory().unwrap();
        crate::persistence::schema::initialize_database(&connection).unwrap();
        let state = crate::test_support::app_state(connection);
        let input = turn("run-cap-plain", "please generate a capability");
        let handled = handle_user_turn(
            &state,
            &input,
            &Sink::default(),
            Arc::new(RunCancellation::default()),
        )
        .await
        .unwrap();
        assert!(!handled);
    }

    #[tokio::test]
    async fn rw_06_malformed_command_finishes_without_a_model_provider() {
        let connection = rusqlite::Connection::open_in_memory().unwrap();
        crate::persistence::schema::initialize_database(&connection).unwrap();
        let state = crate::test_support::app_state(connection);
        let input = turn("run-cap-bad", "/capability generate");
        let events = Sink::default();
        crate::runtime::turns::execute_turn(
            &state,
            &input,
            &events,
            Arc::new(RunCancellation::default()),
            None,
        )
        .await
        .unwrap();
        let content: String = state
            .sqlite_readers
            .read(|connection| {
                connection
                    .query_row(
                        "SELECT content FROM conversation_messages WHERE role = 'assistant' ORDER BY rowid DESC LIMIT 1",
                        [],
                        |row| row.get(0),
                    )
                    .map_err(|error| error.to_string())
            })
            .unwrap();
        assert!(content.contains("not a valid /capability command"));
        let events = events.0.lock().expect("events");
        assert!(events.iter().any(|event| matches!(
            event,
            RuntimeEvent::Started { provider_id, .. } if provider_id == "capability"
        )));
        assert!(!events.iter().any(|event| matches!(
            event,
            RuntimeEvent::Started { provider_id, .. } if provider_id != "capability"
        )));
    }

    #[tokio::test]
    async fn rw_06_inspect_without_a_call_does_not_claim_success() {
        let connection = rusqlite::Connection::open_in_memory().unwrap();
        crate::persistence::schema::initialize_database(&connection).unwrap();
        let state = crate::test_support::app_state(connection);
        let input = turn("run-cap-inspect", "/capability inspect call-missing");
        crate::runtime::turns::execute_turn(
            &state,
            &input,
            &Sink::default(),
            Arc::new(RunCancellation::default()),
            None,
        )
        .await
        .unwrap();
        let content: String = state
            .sqlite_readers
            .read(|connection| {
                connection
                    .query_row(
                        "SELECT content FROM conversation_messages WHERE role = 'assistant' ORDER BY rowid DESC LIMIT 1",
                        [],
                        |row| row.get(0),
                    )
                    .map_err(|error| error.to_string())
            })
            .unwrap();
        assert!(content.contains("not recorded") || content.contains("not authorized"));
        assert!(!content.contains("is recorded against the call owner"));
    }
}
