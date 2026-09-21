use super::*;
pub(crate) fn trim_optional_history_for_tool_follow_up(
    messages: &mut Vec<Value>,
    context_sources: &[Candidate],
) -> bool {
    let mut removed_any = false;
    loop {
        let Some(current_instruction) = messages
            .iter()
            .rposition(|message| message["role"] == "user")
        else {
            return removed_any;
        };
        let Some(index) = (0..current_instruction)
            .find(|&index| matches!(messages[index]["role"].as_str(), Some("user" | "assistant")))
        else {
            return removed_any;
        };
        let mut recomposed = messages.clone();
        recomposed.remove(index);
        if verify_required_wire(&json!({"messages": recomposed}), context_sources).is_err() {
            // Keep a required historical entry and inspect later optional entries instead.
            let mut removed_this_pass = false;
            for later_index in index + 1..current_instruction {
                if !matches!(
                    messages[later_index]["role"].as_str(),
                    Some("user" | "assistant")
                ) {
                    continue;
                }
                let mut later = messages.clone();
                later.remove(later_index);
                if verify_required_wire(&json!({"messages": later}), context_sources).is_ok() {
                    *messages = later;
                    removed_any = true;
                    removed_this_pass = true;
                    break;
                }
            }
            if !removed_this_pass {
                return removed_any;
            }
        } else {
            *messages = recomposed;
            removed_any = true;
        }
    }
}
