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
    ) -> Result<RoundGeneration, ProviderFailureKind> {
        let payload = serde_json::to_vec(&turn_request_body(input))
            .map_err(|_| ProviderFailureKind::Internal)?;
        let oversized =
            payload.len() > crate::runtime::context::generation::MAX_PROVIDER_REQUEST_BYTES;
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
            .transpose();
        if oversized {
            return Err(ProviderFailureKind::RequestTooLarge);
        }
        let generation = generation.map_err(|_| ProviderFailureKind::Internal)?;
        if let Some(generation) = &generation {
            crate::runtime::context::generation_inputs::record(
                generation,
                context.context_health,
                context.context_sources,
                context.context_omissions,
                offered_tools,
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
            .map_err(|_| ProviderFailureKind::Internal)?;
        Ok(())
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

#[cfg(test)]
mod tests {
    use super::*;

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
}
