use super::*;
/// Rebuilds an AgentSession Tool-follow-up by removing only old optional conversation entries
/// from its wrapped base conversation. The current user entry and Tool-result frame are outside
/// the removal range; every candidate removal is validated against the final HTTP body.
pub(crate) fn trim_optional_history_from_follow_up(
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
