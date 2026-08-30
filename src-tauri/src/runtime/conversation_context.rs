use crate::ipc_contract::ConversationMessage;
use crate::memory;

const CONVERSATION_SYSTEM_CONTEXT: &str =
    include_str!("../../../.s11tnext/conversation-respond.txt");

pub(super) fn compose_provider_history(
    conversation_id: &str,
    agent_name: &str,
    user_name: &str,
    input_origin: &str,
    presentation_mode: &str,
    projected: Vec<memory::context_window::ProjectedContextMessage>,
) -> Result<Vec<ConversationMessage>, String> {
    let mut projected = projected.into_iter();
    let policy = projected
        .next()
        .filter(|message| message.role == "system")
        .ok_or_else(|| "Context projection did not begin with its system policy".to_string())?;
    let system_context =
        render_conversation_system_context(agent_name, user_name, input_origin, presentation_mode)?;
    let system_content = format!("{}\n\n{}", system_context.trim(), policy.content.trim());
    let mut history = vec![ConversationMessage {
        id: "context-system-conversation-respond".to_string(),
        conversation_id: conversation_id.to_string(),
        role: "system".to_string(),
        content: system_content,
        created_at: "system".to_string(),
    }];
    history.extend(
        projected
            .enumerate()
            .map(|(index, message)| ConversationMessage {
                id: format!("context-projection-{}", index + 1),
                conversation_id: conversation_id.to_string(),
                role: message.role,
                content: message.content,
                created_at: (index + 1).to_string(),
            }),
    );
    Ok(history)
}

fn render_conversation_system_context(
    agent_name: &str,
    user_name: &str,
    input_origin: &str,
    presentation_mode: &str,
) -> Result<String, String> {
    const PLACEHOLDERS: [&str; 4] = [
        "{{agentNameJson}}",
        "{{userNameJson}}",
        "{{inputOriginJson}}",
        "{{presentationModeJson}}",
    ];
    if PLACEHOLDERS
        .iter()
        .any(|placeholder| CONVERSATION_SYSTEM_CONTEXT.matches(placeholder).count() != 1)
    {
        return Err(
            "Conversation System Context has an invalid runtime placeholder contract".to_string(),
        );
    }
    let encoded = [agent_name, user_name, input_origin, presentation_mode]
        .map(serde_json::to_string)
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| "Conversation runtime data could not be encoded".to_string())?;
    Ok(PLACEHOLDERS.iter().zip(encoded).fold(
        CONVERSATION_SYSTEM_CONTEXT.to_string(),
        |context, (placeholder, value)| context.replace(placeholder, &value),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_history_has_one_leading_system_message() {
        let history = compose_provider_history(
            "conversation-test",
            "こはく",
            "",
            "voice",
            "visual-and-spoken",
            vec![
                memory::context_window::ProjectedContextMessage {
                    role: "system".to_string(),
                    content: "Memory projection policy".to_string(),
                },
                memory::context_window::ProjectedContextMessage {
                    role: "assistant".to_string(),
                    content: "Historical evidence".to_string(),
                },
                memory::context_window::ProjectedContextMessage {
                    role: "user".to_string(),
                    content: "Current request".to_string(),
                },
            ],
        )
        .expect("history composes");
        assert_eq!(history[0].role, "system");
        assert!(history[0]
            .content
            .contains("configured agent name is \"こはく\""));
        assert!(history[0].content.contains("configured user name is \"\""));
        assert!(history[0].content.contains("input origin is \"voice\""));
        assert!(history[0]
            .content
            .contains("presentation mode is \"visual-and-spoken\""));
        assert!(history[0].content.contains("Memory projection policy"));
        assert!(!history[0].content.contains("{{"));
        assert_eq!(
            history
                .iter()
                .filter(|message| message.role == "system")
                .count(),
            1
        );
        assert_eq!(history.last().expect("current request exists").role, "user");
    }

    #[test]
    fn provider_history_json_encodes_runtime_data() {
        let rendered =
            render_conversation_system_context("A \"quoted\" name", "野口", "text", "visual")
                .expect("system context renders");
        assert!(rendered.contains(r#"configured agent name is "A \"quoted\" name""#));
        assert!(rendered.contains(r#"configured user name is "野口""#));
        assert!(!rendered.contains("{{"));
    }
}
