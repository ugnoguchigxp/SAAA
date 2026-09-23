use super::*;

pub(super) struct RequestGeneration {
    handle: Option<crate::runtime::context::generation::GenerationHandle>,
    wire_bytes: usize,
}

impl RequestGeneration {
    pub(super) fn begin(
        context: &ModelStreamContext<'_>,
        body: &Value,
        calls: usize,
        include_world: bool,
        current_instruction: Option<(&str, &str)>,
    ) -> Result<Self, Failure> {
        crate::runtime::context::generation_inputs::verify_required_wire(
            body,
            context.context_sources,
        )
        .map_err(|_| Failure::Internal)?;
        let request_payload = serde_json::to_vec(body).map_err(|_| Failure::Internal)?;
        let has_image = crate::runtime::image_input::body_has_image(body);
        let stored_payload = if has_image {
            crate::runtime::image_input::redact_provider_body(&request_payload)
        } else {
            request_payload.clone()
        };
        let has_required_context = context.context_sources.iter().any(|candidate| {
            candidate.requirement == crate::runtime::context::source::Requirement::Must
        });
        let wire_size = if has_image {
            if request_payload.len() > crate::runtime::image_input::MAX_IMAGE_REQUEST_BYTES {
                crate::runtime::context::generation::FinalWireSize::RequestTooLarge
            } else {
                crate::runtime::context::generation::final_wire_size(
                    stored_payload.len(),
                    has_required_context,
                )
            }
        } else {
            crate::runtime::context::generation::final_wire_size(
                request_payload.len(),
                has_required_context,
            )
        };
        // Earlier user messages are history. The final user message owns this instruction.
        let current_instruction_count = usize::from(
            body["messages"]
                .as_array()
                .and_then(|messages| messages.iter().rev().find(|m| m["role"] == "user"))
                .is_some_and(|m| {
                    message_instruction(&m["content"]).is_some_and(|text| {
                        text.trim()
                            == current_instruction
                                .map(|(_, content)| content)
                                .unwrap_or(&context.input.content)
                                .trim()
                    })
                }),
        );
        match wire_size {
            crate::runtime::context::generation::FinalWireSize::Fits => {}
            crate::runtime::context::generation::FinalWireSize::RequiredContextOverflow => {
                return Err(Failure::RequiredContextOverflow);
            }
            crate::runtime::context::generation::FinalWireSize::RequestTooLarge => {
                return Err(Failure::RequestTooLarge);
            }
        }
        let generation = context
            .output_persistence
            .map(|persistence| {
                let begin = |current_message_id: Option<&str>| match current_message_id {
                    Some(message_id) => persistence.begin_context_generation_for_input(
                        &context.input.run_id,
                        if calls == 0 {
                            "reasoning"
                        } else {
                            "tool-followup"
                        },
                        &stored_payload,
                        &stored_payload,
                        current_instruction_count,
                        message_id,
                    ),
                    None => persistence.begin_context_generation(
                        &context.input.run_id,
                        if calls == 0 {
                            "reasoning"
                        } else {
                            "tool-followup"
                        },
                        &stored_payload,
                        &stored_payload,
                        current_instruction_count,
                    ),
                };
                begin(current_instruction.map(|(message_id, _)| message_id))
            })
            .transpose()
            .map_err(|_| Failure::Internal)?;
        if let Some(generation) = &generation {
            if include_world {
                if let Some(world) = context
                    .output_persistence
                    .and_then(|persistence| persistence.world)
                {
                    world.bind(generation);
                }
            }
            let selected = crate::runtime::context::world::dispatch::selected(
                context.context_sources,
                context.output_persistence.and_then(|p| p.world),
                include_world,
            );
            crate::runtime::context::generation_inputs::record(
                generation,
                context.context_health,
                &selected,
                context.context_omissions,
                body["tools"].as_array().map(Vec::as_slice).unwrap_or(&[]),
                include_world,
            )
            .map_err(|_| Failure::Internal)?;
        }
        Ok(Self {
            handle: generation,
            wire_bytes: request_payload.len(),
        })
    }

    pub(super) fn record_usage(
        &self,
        model: Option<&str>,
        usage: Option<&crate::runtime::context::usage::ProviderUsage>,
        source: crate::runtime::context::usage::UsageSource,
        timings: &crate::runtime::context::usage::UsageTimings,
        prefix_match_bytes: Option<i64>,
    ) {
        let Some(handle) = &self.handle else {
            return;
        };
        let usage = usage.cloned().unwrap_or_default();
        let _ = handle.writer_record(
            model,
            &usage,
            source,
            self.wire_bytes,
            timings,
            prefix_match_bytes,
        );
    }

    pub(super) fn complete(&self) -> Result<(), Failure> {
        self.handle
            .as_ref()
            .map(|generation| generation.complete())
            .transpose()
            .map_err(context_dependency_failure)?;
        Ok(())
    }

    pub(super) fn revalidate_before_tool(&self) -> Result<(), Failure> {
        self.handle
            .as_ref()
            .map(|generation| generation.revalidate_dependencies())
            .transpose()
            .map(|_| ())
            .map_err(context_dependency_failure)
    }

    pub(super) fn fail(&self, reason: &str) {
        if let Some(generation) = &self.handle {
            let _ = generation.fail(reason);
        }
    }

    pub(super) fn finish_error(&self, error: Failure) {
        if error == Failure::Cancelled {
            if let Some(generation) = &self.handle {
                let _ = generation.cancel();
            }
        } else {
            self.fail(error.as_str());
        }
    }
}

fn message_instruction(content: &Value) -> Option<&str> {
    if let Some(text) = content.as_str() {
        return Some(text);
    }
    content.as_array().and_then(|parts| {
        parts.iter().find_map(|part| {
            (part["type"] == "text")
                .then(|| part["text"].as_str())
                .flatten()
        })
    })
}

fn context_dependency_failure(error: String) -> Failure {
    if error.contains("scope dependency changed") {
        Failure::ContextScopeChanged
    } else {
        Failure::RequiredContextUnavailable
    }
}
