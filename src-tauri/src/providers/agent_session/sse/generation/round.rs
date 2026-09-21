use super::*;
pub(crate) struct RoundGeneration(
    pub(super) Option<crate::runtime::context::generation::GenerationHandle>,
);

impl RoundGeneration {
    pub(crate) fn complete(&self) -> Result<(), ProviderFailureKind> {
        self.0
            .as_ref()
            .map(|generation| generation.complete())
            .transpose()
            .map_err(context_dependency_failure)?;
        Ok(())
    }

    pub(crate) fn revalidate_before_tool(&self) -> Result<(), ProviderFailureKind> {
        self.0
            .as_ref()
            .map(|generation| generation.revalidate_dependencies())
            .transpose()
            .map(|_| ())
            .map_err(context_dependency_failure)
    }

    pub(crate) fn cancel(&self) {
        if let Some(generation) = &self.0 {
            let _ = generation.cancel();
        }
    }

    pub(crate) fn fail(&self, reason: &str) {
        if let Some(generation) = &self.0 {
            let _ = generation.fail(reason);
        }
    }

    pub(crate) fn finish_outcome(&self, outcome: &ProviderAttemptOutcome) {
        match outcome {
            ProviderAttemptOutcome::Cancelled { .. } => self.cancel(),
            ProviderAttemptOutcome::Failed { kind, .. } => self.fail(kind.as_str()),
            ProviderAttemptOutcome::Completed { .. } => {}
        }
    }
}

fn context_dependency_failure(error: String) -> ProviderFailureKind {
    if error.contains("scope dependency changed") {
        ProviderFailureKind::ContextScopeChanged
    } else {
        ProviderFailureKind::RequiredContextUnavailable
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::event_hub::RuntimeEventSender;
    use std::sync::Arc;

    #[derive(Clone)]
    struct Sink;

    impl RuntimeEventSender for Sink {
        fn clone_box(&self) -> Box<dyn RuntimeEventSender> {
            Box::new(self.clone())
        }

        fn send(&self, _: crate::ipc_contract::RuntimeEvent) -> tauri::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn tool_follow_up_carries_the_original_instruction_once() {
        let envelope = Envelope::new(
            r#"{"type":"saaa.conversation.v1","messages":[{"role":"user","content":"current"}]}"#,
        );
        let follow_up = envelope.follow_up(r#"{"name":"tool","result":{"ok":true}}"#);
        let value: Value = serde_json::from_str(&follow_up).unwrap();
        assert_eq!(
            value["conversation"]["messages"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|message| message["role"] == "user")
                .count(),
            1
        );
        assert_eq!(value["toolResultAuthority"], "none");
    }

    #[test]
    fn manifest_payload_matches_the_actual_http_body_shape() {
        let input = "x".repeat(crate::runtime::context::generation::MAX_PROVIDER_REQUEST_BYTES);
        let payload = serde_json::to_vec(&turn_request_body(&input)).unwrap();
        assert!(payload.len() > crate::runtime::context::generation::MAX_PROVIDER_REQUEST_BYTES);
        assert_eq!(
            serde_json::from_slice::<Value>(&payload).unwrap()["input"][0]["text"],
            input
        );
    }

    #[test]
    fn tool_follow_up_trim_keeps_required_wrapped_history() {
        let required = crate::runtime::context::source::Candidate::untrusted(
            "required".into(),
            "personal-state",
            vec![],
            crate::runtime::context::source::Requirement::Must,
            "state".into(),
            1,
            1,
            "external send is prohibited".into(),
        );
        let envelope = Envelope::new(
            r#"{"type":"saaa.conversation.v1","messages":[{"role":"user","content":"old"},{"role":"assistant","content":"external send is prohibited"},{"role":"assistant","content":"old answer"},{"role":"user","content":"current"}]}"#,
        );
        let mut follow_up = envelope.follow_up(r#"{"result":"tool"}"#);

        assert!(trim_optional_history_from_follow_up(
            &mut follow_up,
            std::slice::from_ref(&required),
        ));
        let value: Value = serde_json::from_str(&follow_up).unwrap();
        let messages = value["conversation"]["messages"].as_array().unwrap();
        assert!(messages
            .iter()
            .any(|message| message["content"] == required.content));
        assert!(messages
            .iter()
            .any(|message| message["content"] == "current"));
        assert!(!messages.iter().any(|message| message["content"] == "old"));
        assert!(!messages
            .iter()
            .any(|message| message["content"] == "old answer"));
        assert_eq!(value["toolResult"]["result"], "tool");
    }

    #[test]
    fn tool_follow_up_trim_recovers_an_agent_session_wire_overflow() {
        let required = crate::runtime::context::source::Candidate::untrusted(
            "required".into(),
            "personal-state",
            vec![],
            crate::runtime::context::source::Requirement::Must,
            "state".into(),
            1,
            1,
            "external send is prohibited".into(),
        );
        let base = json!({
            "type":"saaa.conversation.v1",
            "messages":[
                {"role":"user","content":"x".repeat(30_000)},
                {"role":"assistant","content":"external send is prohibited"},
                {"role":"assistant","content":"y".repeat(30_000)},
                {"role":"user","content":"current"}
            ]
        })
        .to_string();
        let envelope = Envelope::new(&base);
        let mut follow_up = envelope.follow_up(&json!({"result":"z".repeat(20_000)}).to_string());
        let size = |input: &str| serde_json::to_vec(&turn_request_body(input)).unwrap().len();
        assert!(
            size(&follow_up) > crate::runtime::context::generation::MAX_PROVIDER_CONTEXT_WIRE_BYTES
        );

        assert!(trim_optional_history_from_follow_up(
            &mut follow_up,
            std::slice::from_ref(&required),
        ));
        assert!(
            size(&follow_up)
                <= crate::runtime::context::generation::MAX_PROVIDER_CONTEXT_WIRE_BYTES
        );
        assert!(
            crate::runtime::context::generation_inputs::verify_required_wire(
                &turn_request_body(&follow_up),
                &[required],
            )
            .is_ok()
        );
    }

    #[test]
    fn scope_change_before_agent_session_completion_has_a_typed_context_failure() {
        let connection = rusqlite::Connection::open_in_memory().unwrap();
        crate::persistence::schema::initialize_database(&connection).unwrap();
        connection
            .execute(
                "INSERT INTO conversations(id,task_mode,created_at,updated_at)
                 VALUES('fixture','conversation','1','1')",
                [],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO conversation_messages VALUES('source','fixture','user','hello','1')",
                [],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO runtime_runs(id,conversation_id,route_kind,status,input_message_id,started_at)
                 VALUES('run','fixture','conversation.respond','running','source','1')",
                [],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO context_scopes(scope_key,kind,opaque_id,state,created_at)
                 VALUES('scope','project','opaque','active','1')",
                [],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO context_scope_epochs(scope_key,epoch) VALUES('scope',0)",
                [],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO runtime_scope_resolutions(run_id,status,focus_scope_key,scope_digest,resolved_at)
                 VALUES('run','resolved','scope',?1,'1')",
                [&"d".repeat(64)],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO runtime_run_scopes(run_id,scope_key,relation,source,epoch)
                 VALUES('run','scope','current','runtime',0)",
                [],
            )
            .unwrap();
        let state = crate::test_support::app_state(connection);
        let session = crate::begin_provider_session(
            &state,
            "run",
            "fixture",
            "openai-compatible",
            &"a".repeat(64),
        )
        .unwrap();
        let input = crate::StartTurnInput {
            run_id: "run".into(),
            conversation_id: "fixture".into(),
            content: "hello".into(),
            workspace_path: None,
            retry_input_message_id: None,
            source_id: None,
            scope_refs: Vec::new(),
            input_origin: "text".into(),
            presentation_mode: "visual".into(),
        };
        let context = ModelStreamContext {
            reasoning_effort: "medium",
            max_output_tokens: 64,
            input: &input,
            on_event: &Sink,
            cancellation: Arc::default(),
            context_health: "green",
            context_sources: &[],
            context_omissions: &[],
            output_persistence: Some(crate::ProviderOutputPersistence {
                state: &state,
                session_id: &session,
                world: None,
            }),
        };
        let envelope = Envelope::new(
            r#"{"type":"saaa.conversation.v1","messages":[{"role":"user","content":"hello"}]}"#,
        );
        let generation = envelope
            .begin(
                &context,
                0,
                r#"{"type":"saaa.conversation.v1","messages":[{"role":"user","content":"hello"}]}"#,
                &[],
                false,
            )
            .unwrap();
        state
            .sqlite_writer
            .write(|connection| {
                connection
                    .execute(
                        "UPDATE context_scope_epochs SET epoch=epoch+1 WHERE scope_key='scope'",
                        [],
                    )
                    .map_err(crate::database_error)?;
                Ok(())
            })
            .unwrap();
        assert_eq!(
            generation.complete(),
            Err(ProviderFailureKind::ContextScopeChanged)
        );
    }
}
