use crate::ipc_contract::ConversationMessage;
use crate::memory;
use crate::persistence::settings::regional_preferences::RegionalPreferences;

const CONVERSATION_SYSTEM_CONTEXT: &str =
    include_str!("../../../.s11tnext/conversation-respond.txt");

pub(super) fn compose_provider_history(
    conversation_id: &str,
    agent_name: &str,
    user_name: &str,
    regional: &RegionalPreferences,
    input_origin: &str,
    presentation_mode: &str,
    projected: Vec<memory::context_window::ProjectedContextMessage>,
) -> Result<Vec<ConversationMessage>, String> {
    let mut projected = projected.into_iter();
    let policy = projected
        .next()
        .filter(|message| message.role == "system")
        .ok_or_else(|| "Context projection did not begin with its system policy".to_string())?;
    let system_context = render_conversation_system_context(
        agent_name,
        user_name,
        regional,
        input_origin,
        presentation_mode,
    )?;
    let mut system_content = format!("{}\n\n{}", system_context.trim(), policy.content.trim());
    let mut history = Vec::new();
    for (index, message) in projected.enumerate() {
        if message.role == memory::context_window::EVIDENCE_ROLE {
            system_content.push_str("\n\n");
            system_content.push_str(message.content.trim());
            continue;
        }
        history.push(ConversationMessage {
            parts: None,
            id: format!("context-projection-{}", index + 1),
            conversation_id: conversation_id.to_string(),
            role: message.role,
            content: message.content,
            created_at: (index + 1).to_string(),
        });
    }
    history.insert(
        0,
        ConversationMessage {
            parts: None,
            id: "context-system-conversation-respond".to_string(),
            conversation_id: conversation_id.to_string(),
            role: "system".to_string(),
            content: system_content,
            created_at: "system".to_string(),
        },
    );
    Ok(history)
}

pub(super) fn render_fixed_system_context(
    agent_name: &str,
    user_name: &str,
    regional: &RegionalPreferences,
    policy: &str,
) -> Result<String, String> {
    let head = render_conversation_system_context(agent_name, user_name, regional, "", "")?;
    Ok(format!("{}\n\n{}", head.trim(), policy.trim()))
}

fn render_conversation_system_context(
    agent_name: &str,
    user_name: &str,
    regional: &RegionalPreferences,
    input_origin: &str,
    presentation_mode: &str,
) -> Result<String, String> {
    const PLACEHOLDERS: [&str; 5] = [
        "{{agentNameJson}}",
        "{{userNameJson}}",
        "{{regionalPreferencesJson}}",
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
    let encoded = [
        serde_json::to_string(agent_name),
        serde_json::to_string(user_name),
        serde_json::to_string(regional),
        serde_json::to_string(input_origin),
        serde_json::to_string(presentation_mode),
    ]
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
            "c",
            "n",
            "",
            &RegionalPreferences::default(),
            "voice",
            "visual-and-spoken",
            vec![
                memory::context_window::ProjectedContextMessage {
                    role: "system".to_string(),
                    content: "p".to_string(),
                },
                memory::context_window::ProjectedContextMessage {
                    role: "user".to_string(),
                    content: "u".to_string(),
                },
            ],
        )
        .expect("history composes");
        assert_eq!(
            history
                .iter()
                .map(|message| message.role.as_str())
                .collect::<Vec<_>>(),
            ["system", "user"]
        );
        assert!(history[0].content.contains("agent=\"n\""));
        assert!(history[0].content.contains("user=\"\""));
        assert!(history[0].content.contains(r#""currency":"JPY""#));
        assert!(history[0].content.contains("inputOrigin=\"voice\""));
        assert!(history[0]
            .content
            .contains("presentationMode=\"visual-and-spoken\""));
        assert!(history[0].content.contains('\n') && history[0].content.contains('p'));
        assert!(!history[0].content.contains("{{"));
        assert_eq!(history[1].content, "u");
    }

    #[test]
    fn evidence_role_is_folded_into_system_and_never_a_chat_turn() {
        let history = compose_provider_history(
            "c",
            "n",
            "",
            &RegionalPreferences::default(),
            "voice",
            "visual-and-spoken",
            vec![
                memory::context_window::ProjectedContextMessage {
                    role: "system".to_string(),
                    content: "p".to_string(),
                },
                memory::context_window::ProjectedContextMessage {
                    role: memory::context_window::EVIDENCE_ROLE.to_string(),
                    content: "e".to_string(),
                },
                memory::context_window::ProjectedContextMessage {
                    role: "user".to_string(),
                    content: "u".to_string(),
                },
            ],
        )
        .expect("history composes");
        assert_eq!(
            history
                .iter()
                .map(|message| message.role.as_str())
                .collect::<Vec<_>>(),
            ["system", "user"]
        );
        assert!(history[0].content.contains('e'));
        assert_eq!(history[1].content, "u");
    }

    #[test]
    fn provider_history_json_encodes_runtime_data() {
        let regional = RegionalPreferences {
            language: "ja".to_string(),
            time_zone: "Asia/Tokyo".to_string(),
            length_unit: "metric".to_string(),
            weight_unit: "kilogram".to_string(),
            currency: "JPY".to_string(),
        };
        let rendered = render_conversation_system_context(
            "A \"quoted\" name",
            "",
            &regional,
            "text",
            "visual",
        )
        .expect("system context renders");
        assert!(rendered.contains(r#"agent="A \"quoted\" name""#));
        assert!(rendered.contains(r#"user="""#));
        assert!(rendered.contains(
            r#"regional={"language":"ja","timeZone":"Asia/Tokyo","lengthUnit":"metric","weightUnit":"kilogram","currency":"JPY"}"#
        ));
        assert!(!rendered.contains("{{"));
    }
}
