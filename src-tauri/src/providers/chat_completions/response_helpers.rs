use super::*;

pub(super) fn mark_started(
    context: &ModelStreamContext<'_>,
    started: &mut bool,
) -> Result<(), Failure> {
    if !*started {
        if let Some(persistence) = context.output_persistence {
            persistence.mark_started().map_err(|_| Failure::Internal)?;
        }
        *started = true;
    }
    Ok(())
}

pub(super) fn json_events(bytes: &[u8]) -> Result<Vec<String>, Failure> {
    let mut value: Value = serde_json::from_slice(bytes).map_err(|_| Failure::Protocol)?;
    if value.get("error").is_some() {
        return Err(Failure::Upstream);
    }
    let choices = value
        .get_mut("choices")
        .and_then(Value::as_array_mut)
        .ok_or(Failure::Protocol)?;
    if choices.len() != 1 {
        return Err(Failure::Protocol);
    }
    let choice = choices[0].as_object_mut().ok_or(Failure::Protocol)?;
    let mut message = choice
        .remove("message")
        .filter(Value::is_object)
        .ok_or(Failure::Protocol)?;
    if let Some(calls) = message.get_mut("tool_calls").filter(|v| !v.is_null()) {
        for (index, call) in calls
            .as_array_mut()
            .ok_or(Failure::Protocol)?
            .iter_mut()
            .enumerate()
        {
            call.as_object_mut()
                .ok_or(Failure::Protocol)?
                .insert("index".into(), json!(index));
        }
    }
    choice.insert("delta".into(), message);
    Ok(vec![value.to_string(), "[DONE]".to_string()])
}
