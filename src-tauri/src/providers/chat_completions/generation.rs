use super::*;

pub(super) struct RequestGeneration(Option<crate::runtime::context::generation::GenerationHandle>);

impl RequestGeneration {
    pub(super) fn begin(
        context: &ModelStreamContext<'_>,
        body: &Value,
        calls: usize,
        include_world: bool,
    ) -> Result<Self, Failure> {
        crate::runtime::context::generation_inputs::verify_required_wire(
            body,
            context.context_sources,
        )
        .map_err(|_| Failure::Internal)?;
        let request_payload = serde_json::to_vec(body).map_err(|_| Failure::Internal)?;
        let oversized =
            request_payload.len() > crate::runtime::context::generation::MAX_PROVIDER_REQUEST_BYTES;
        let current_instruction_count = body["messages"]
            .as_array()
            .map(|messages| {
                messages
                    .iter()
                    .filter(|message| message["role"] == "user")
                    .count()
            })
            .unwrap_or_default();
        let generation = context
            .output_persistence
            .map(|persistence| {
                persistence.begin_context_generation(
                    &context.input.run_id,
                    if calls == 0 {
                        "reasoning"
                    } else {
                        "tool-followup"
                    },
                    &request_payload,
                    &request_payload,
                    current_instruction_count,
                )
            })
            .transpose();
        if oversized {
            return Err(Failure::RequestTooLarge);
        }
        let generation = generation.map_err(|_| Failure::Internal)?;
        if let Some(generation) = &generation {
            if include_world {
                if let Some(world) = context
                    .output_persistence
                    .and_then(|persistence| persistence.world)
                {
                    world.bind(generation);
                }
            }
            crate::runtime::context::generation_inputs::record(
                generation,
                context.context_health,
                context.context_sources,
                context.context_omissions,
                body["tools"].as_array().map(Vec::as_slice).unwrap_or(&[]),
                include_world,
            )
            .map_err(|_| Failure::Internal)?;
        }
        Ok(Self(generation))
    }

    pub(super) fn complete(&self) -> Result<(), Failure> {
        self.0
            .as_ref()
            .map(|generation| generation.complete())
            .transpose()
            .map_err(|_| Failure::Internal)?;
        Ok(())
    }

    pub(super) fn fail(&self, reason: &str) {
        if let Some(generation) = &self.0 {
            let _ = generation.fail(reason);
        }
    }

    pub(super) fn finish_error(&self, error: Failure) {
        if error == Failure::Cancelled {
            if let Some(generation) = &self.0 {
                let _ = generation.cancel();
            }
        } else {
            self.fail(error.as_str());
        }
    }
}
