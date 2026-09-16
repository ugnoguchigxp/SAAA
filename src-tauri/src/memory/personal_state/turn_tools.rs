use serde_json::Value;
pub fn decode(
    response: &Value,
    offered: &[Value],
    attempt: &str,
) -> Result<crate::runtime::agent_tools::AgentToolCall, String> {
    let calls = response["choices"][0]["message"]["tool_calls"]
        .as_array()
        .ok_or("personal-tool-schema")?;
    if calls.len() != 1 || calls[0]["type"] != "function" {
        return Err("personal-tool-schema".into());
    }
    let name = calls[0]["function"]["name"]
        .as_str()
        .ok_or("personal-tool-schema")?;
    let args = calls[0]["function"]["arguments"]
        .as_str()
        .filter(|v| v.len() <= 70000)
        .ok_or("personal-tool-schema")?;
    if !offered.iter().any(|d| d["function"]["name"] == name) {
        return Err("personal-tool-not-offered".into());
    }
    Ok(crate::runtime::agent_tools::AgentToolCall {
        id: format!("{attempt}-tool"),
        name: name.into(),
        arguments: args.into(),
    })
}
