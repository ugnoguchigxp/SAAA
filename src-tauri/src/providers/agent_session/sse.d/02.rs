#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_crlf_events_and_ignores_heartbeats() {
        let mut buffer = b": heartbeat\r\n\r\nid: c1\r\nevent: message.delta\r\ndata: {\"type\":\"message.delta\",\"session_id\":\"ags_1\",\"turn_id\":\"agt_1\",\"cursor\":\"c1\",\"data\":{\"text\":\"ok\"}}\r\n\r\n".to_vec();
        assert!(parse_event(&take_event(&mut buffer).expect("heartbeat"))
            .unwrap()
            .is_none());
        let parsed = parse_event(&take_event(&mut buffer).expect("event"))
            .unwrap()
            .expect("payload");
        assert_eq!(parsed.event_name.as_deref(), Some("message.delta"));
        assert_eq!(parsed.payload.data["text"], "ok");
        assert!(buffer.is_empty());
    }

    #[test]
    fn serializes_role_preserving_conversation_input() {
        let message = |role: &str, content: &str| ConversationMessage {
            parts: None,
            id: "id".to_string(),
            conversation_id: "conversation".to_string(),
            role: role.to_string(),
            content: content.to_string(),
            created_at: "now".to_string(),
        };
        let input = render_turn_input(&[
            message("system", "policy"),
            message("assistant", "prior"),
            message("user", "question"),
        ])
        .unwrap();
        let value: Value = serde_json::from_str(&input).unwrap();
        assert_eq!(value["messages"][0]["role"], "system");
        assert_eq!(value["messages"][2]["content"], "question");
    }

    #[test]
    fn folds_delta_and_terminal_events_into_a_completed_attempt() {
        let input = crate::StartTurnInput {
            run_id: "run".to_string(),
            conversation_id: "conversation".to_string(),
            content: "question".to_string(),
            workspace_path: None,
            retry_input_message_id: None,
            source_id: None,
            scope_refs: Vec::new(),
            input_origin: "text".to_string(),
            presentation_mode: "visual".to_string(),
        };
        let sink = tauri::ipc::Channel::new(|_| Ok(()));
        let cancellation = std::sync::Arc::new(crate::RunCancellation::default());
        let context = ModelStreamContext {
            reasoning_effort: "medium",
            max_output_tokens: 64,
            input: &input,
            on_event: &sink,
            cancellation,
            context_health: "green",
            context_sources: &[],
            context_omissions: &[],
            output_persistence: None,
        };
        let session = SessionResponse {
            id: "ags_1".to_string(),
            events_url: Some("/events".to_string()),
        };
        let event = |event_type: &str, cursor: &str, data: Value| ParsedEvent {
            event_name: Some(event_type.to_string()),
            id: Some(cursor.to_string()),
            payload: AgentEvent {
                event_type: event_type.to_string(),
                session_id: "ags_1".to_string(),
                turn_id: Some("agt_1".to_string()),
                cursor: Some(cursor.to_string()),
                data,
            },
        };
        let mut state = StreamState::default();
        assert!(accept_event(
            event("message.delta", "c1", json!({ "text": "ready" })),
            &session,
            "agt_1",
            &context,
            &mut state,
        )
        .is_none());
        // Some SSE servers replay the last event itself when reconnecting.
        assert!(accept_event(
            event("message.delta", "c1", json!({ "text": "ready" })),
            &session,
            "agt_1",
            &context,
            &mut state,
        )
        .is_none());
        assert_eq!(state.content, "ready");
        let terminal = accept_event(
            event("turn.completed", "c2", json!({})),
            &session,
            "agt_1",
            &context,
            &mut state,
        );
        assert!(matches!(
            terminal,
            Some(ReadResult::Terminal(ProviderAttemptOutcome::Completed { content, .. }))
                if content == "ready"
        ));
    }
}
