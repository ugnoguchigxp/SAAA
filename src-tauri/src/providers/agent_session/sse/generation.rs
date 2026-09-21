use super::*;

pub(super) struct Envelope(String);

impl Envelope {
    pub(super) fn new(input: &str) -> Self {
        Self(input.to_owned())
    }

    pub(super) fn begin(
        &self,
        context: &ModelStreamContext<'_>,
        round: usize,
        input: &str,
        offered_tools: &[Value],
        include_world: bool,
    ) -> Result<RoundGeneration, ProviderFailureKind> {
        let body = turn_request_body(input);
        crate::runtime::context::generation_inputs::verify_required_wire(
            &body,
            context.context_sources,
        )
        .map_err(|_| ProviderFailureKind::Internal)?;
        let payload = serde_json::to_vec(&body).map_err(|_| ProviderFailureKind::Internal)?;
        let wire_size = crate::runtime::context::generation::final_wire_size(
            payload.len(),
            context.context_sources.iter().any(|candidate| {
                candidate.requirement == crate::runtime::context::source::Requirement::Must
            }),
        );
        match wire_size {
            crate::runtime::context::generation::FinalWireSize::Fits => {}
            crate::runtime::context::generation::FinalWireSize::RequiredContextOverflow => {
                return Err(ProviderFailureKind::RequiredContextOverflow);
            }
            crate::runtime::context::generation::FinalWireSize::RequestTooLarge => {
                return Err(ProviderFailureKind::RequestTooLarge);
            }
        }
        let generation = context
            .output_persistence
            .map(|persistence| {
                persistence.begin_context_generation(
                    &context.input.run_id,
                    if round == 0 {
                        "reasoning"
                    } else {
                        "tool-followup"
                    },
                    &payload,
                    &payload,
                    1,
                )
            })
            .transpose()
            .map_err(|_| ProviderFailureKind::Internal)?;
        if let Some(generation) = &generation {
            if include_world {
                if let Some(world) = context
                    .output_persistence
                    .and_then(|persistence| persistence.world)
                {
                    world.bind(generation);
                }
            }
            crate::runtime::context::generation_inputs::record(
                generation,
                context.context_health,
                context.context_sources,
                context.context_omissions,
                offered_tools,
                include_world,
            )
            .map_err(|_| ProviderFailureKind::Internal)?;
        }
        Ok(RoundGeneration(generation))
    }

    pub(super) fn follow_up(&self, tool_result: &str) -> String {
        json!({
            "type": "saaa.conversation.tool-followup.v1",
            "conversation": serde_json::from_str::<Value>(&self.0).unwrap_or(Value::String(self.0.clone())),
            "toolResult": serde_json::from_str::<Value>(tool_result).unwrap_or(Value::String(tool_result.to_owned())),
            "toolResultAuthority": "none"
        })
        .to_string()
    }
}

/// Rebuilds an AgentSession Tool-follow-up by removing only old optional conversation entries
/// from its wrapped base conversation. The current user entry and Tool-result frame are outside
/// the removal range; every candidate removal is validated against the final HTTP body.
pub(super) fn trim_optional_history_from_follow_up(
    input: &mut String,
    context_sources: &[crate::runtime::context::source::Candidate],
) -> bool {
    let Ok(mut value) = serde_json::from_str::<Value>(input) else {
        return false;
    };
    let mut removed_any = false;
    loop {
        let Some(current_instruction) = conversation_messages(&mut value).and_then(|messages| {
            messages
                .iter()
                .rposition(|message| message["role"] == "user")
        }) else {
            return removed_any;
        };
        let removable = conversation_messages(&mut value).and_then(|messages| {
            (0..current_instruction).find(|&index| {
                matches!(messages[index]["role"].as_str(), Some("user" | "assistant"))
            })
        });
        let Some(index) = removable else {
            return removed_any;
        };
        let original = value.clone();
        let removed = conversation_messages(&mut value)
            .and_then(|messages| (index < messages.len()).then(|| messages.remove(index)));
        if removed.is_none() {
            return removed_any;
        }
        let valid = serde_json::to_string(&value).ok().is_some_and(|rendered| {
            crate::runtime::context::generation_inputs::verify_required_wire(
                &turn_request_body(&rendered),
                context_sources,
            )
            .is_ok()
        });
        if valid {
            *input = serde_json::to_string(&value).expect("already serialized");
            removed_any = true;
            continue;
        }
        value = original;
        let mut removed_this_pass = false;
        for later_index in index + 1..current_instruction {
            let is_removable = conversation_messages(&mut value).is_some_and(|messages| {
                later_index < messages.len()
                    && matches!(
                        messages[later_index]["role"].as_str(),
                        Some("user" | "assistant")
                    )
            });
            if !is_removable {
                continue;
            }
            let original = value.clone();
            conversation_messages(&mut value)
                .expect("messages were found above")
                .remove(later_index);
            let valid = serde_json::to_string(&value).ok().is_some_and(|rendered| {
                crate::runtime::context::generation_inputs::verify_required_wire(
                    &turn_request_body(&rendered),
                    context_sources,
                )
                .is_ok()
            });
            if valid {
                *input = serde_json::to_string(&value).expect("already serialized");
                removed_any = true;
                removed_this_pass = true;
                break;
            }
            value = original;
        }
        if !removed_this_pass {
            return removed_any;
        }
    }
}

fn conversation_messages(value: &mut Value) -> Option<&mut Vec<Value>> {
    if value["type"] == "saaa.conversation.v1" {
        return value.get_mut("messages")?.as_array_mut();
    }
    value
        .get_mut("conversation")
        .and_then(conversation_messages)
}

pub(super) fn turn_request_body(input: &str) -> Value {
    json!({ "input": [{ "type": "text", "text": input }] })
}

pub(super) struct RoundGeneration(Option<crate::runtime::context::generation::GenerationHandle>);

impl RoundGeneration {
    pub(super) fn complete(&self) -> Result<(), ProviderFailureKind> {
        self.0
            .as_ref()
            .map(|generation| generation.complete())
            .transpose()
            .map_err(context_dependency_failure)?;
        Ok(())
    }

    pub(super) fn revalidate_before_tool(&self) -> Result<(), ProviderFailureKind> {
        self.0
            .as_ref()
            .map(|generation| generation.revalidate_dependencies())
            .transpose()
            .map(|_| ())
            .map_err(context_dependency_failure)
    }

    pub(super) fn cancel(&self) {
        if let Some(generation) = &self.0 {
            let _ = generation.cancel();
        }
    }

    pub(super) fn fail(&self, reason: &str) {
        if let Some(generation) = &self.0 {
            let _ = generation.fail(reason);
        }
    }

    pub(super) fn finish_outcome(&self, outcome: &ProviderAttemptOutcome) {
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
