//! Component fixture: current authorized World source + current Context Broker.
//! This is not the removed conversation executor. Queue dispatch has its own contract tests.
#![cfg(test)]
use super::{source::*, turn::*};
use crate::memory::context_window::ContextWindow;
use crate::memory::personal_state::world::runtime_frame::{FrameRequest, GraphRequest, WorldFrameService};
use crate::runtime::context::{broker::{self, BrokerInput, Envelope}, scope::ScopeSnapshot, source::Candidate};
use saaa_personal_state_core::{AccessRequest, Classification, Purpose};
use std::{collections::BTreeSet, sync::Arc};

pub(crate) struct ComposedFixture {
    pub envelope: Envelope,
    pub world: Option<WorldLive>,
}
#[allow(clippy::too_many_arguments)]
pub(crate) fn compose_fixture(
    enabled: bool, service: Option<Arc<WorldFrameService>>, principal: &str,
    policy_revision: u64, graph_request: Option<GraphRequest>, run_id: &str,
    scope: &ScopeSnapshot, base: ContextWindow, mut candidates: Vec<Candidate>,
    allowed_scope_keys: BTreeSet<String>,
) -> Result<ComposedFixture, String> {
    let mut ready = None;
    if enabled {
        if let (Some(service), Ok(root)) = (service, explicit_project(scope)) {
            let refs = runtime_refs(scope);
            if !refs.is_empty() || graph_request.is_some() {
                let explicit = graph_request.as_ref().is_some_and(|g| g.explicit_question);
                let request = WorldSourceRequest { frame_request: FrameRequest {
                    run_id, project_scope: &root,
                    access: AccessRequest { principal, scope: "primary", task_request: Some(&root), purpose: Purpose::Reasoning,
                        max_classification: Classification::Confidential, policy_revision, authorized: true },
                    runtime_refs: refs, graph_request, max_bytes: 32000, ttl_ms: 1000,
                }};
                let outcome = if explicit { prepare_explicit_question_candidate(&service, request, scope) }
                    else { prepare_candidate(&service, request, scope) };
                if let WorldSourceOutcome::Ready(prepared) = outcome {
                    let prepared = with_kind(prepared, WORLD_KIND);
                    candidates.push(prepared.candidate().clone());
                    ready = Some((service, prepared.into_prepared()));
                }
            }
        }
    }
    let without = broker::compose(BrokerInput { base: base.clone(), candidates: candidates.iter().filter(|c| c.source_kind != WORLD_KIND).cloned().collect(), source_warning: None, allowed_scope_keys: allowed_scope_keys.clone() })?;
    let envelope = broker::compose(BrokerInput { base, candidates, source_warning: None, allowed_scope_keys })?;
    let world = if envelope.selected.iter().any(|c| c.source_kind == WORLD_KIND) {
        ready.map(|(service, prepared)| WorldLive::live(service, prepared, envelope.combined_block.clone().map(|with_world| WorldBlocks { with_world, without_world: without.combined_block })))
    } else { None };
    Ok(ComposedFixture { envelope, world })
}
