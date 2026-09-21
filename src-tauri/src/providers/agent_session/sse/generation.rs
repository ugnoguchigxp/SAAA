use super::*;
mod round;
mod trim;
use round::RoundGeneration;
pub(super) use trim::trim_optional_history_from_follow_up;

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
            let candidates = crate::runtime::context::world::dispatch::selected(
                context.context_sources,
                context.output_persistence.and_then(|p| p.world),
                include_world,
            );
            crate::runtime::context::generation_inputs::record(
                generation,
                context.context_health,
                &candidates,
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

pub(super) fn turn_request_body(input: &str) -> Value {
    json!({ "input": [{ "type": "text", "text": input }] })
}
