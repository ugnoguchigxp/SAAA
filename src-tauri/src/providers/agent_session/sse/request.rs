//! AgentSession initial request serialization and transport.
use super::*;

pub(super) async fn start_turn(
    client: &Client,
    provider: &AgentSessionProviderSettings,
    session: &SessionResponse,
    input: &str,
    api_key: Option<&str>,
) -> Result<TurnResponse, ProviderFailureKind> {
    let request = authorized(
        client.post(session_operation_url(provider, &session.id, "turns")?),
        api_key,
    )
    .header("Idempotency-Key", idempotency_key())
    .json(&generation::turn_request_body(input));
    let turn: TurnResponse = super::super::read_json_response(send(request).await?).await?;
    if !safe_remote_id(&turn.id)
        || turn
            .session_id
            .as_deref()
            .is_some_and(|session_id| session_id != session.id)
    {
        return Err(ProviderFailureKind::Protocol);
    }
    Ok(turn)
}

pub(super) fn render_turn_input(
    history: &[ConversationMessage],
) -> Result<String, ProviderFailureKind> {
    let messages = history
        .iter()
        .filter_map(|message| {
            let role = match message.role.as_str() {
                "system" => "system",
                "assistant" => "assistant",
                "user" | "transcript" => "user",
                _ => return None,
            };
            Some(json!({ "role": role, "content": message.content }))
        })
        .collect::<Vec<_>>();
    let input = serde_json::to_string(&json!({
        "type": "saaa.conversation.v1",
        "messages": messages,
    }))
    .map_err(|_| ProviderFailureKind::Internal)?;
    if input.is_empty() || input.len() > 1_000_000 {
        return Err(ProviderFailureKind::RequestTooLarge);
    }
    Ok(input)
}
