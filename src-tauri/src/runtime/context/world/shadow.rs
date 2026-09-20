//! Same-process World shadow comparison against the real Context Broker (S5/S6).
use super::super::broker::{self, BrokerInput, Envelope};
use super::super::source::Candidate;
use super::source::{
    omission_from_frame, omission_from_validity, prepare_candidate, WorldOmission,
    WorldSourceOutcome, WorldSourceRequest, WORLD_SHADOW_KIND,
};
use crate::memory::context_window::ContextWindow;
use crate::memory::personal_state::world::runtime_frame::WorldFrameService;
use crate::runtime::context::scope::ScopeSnapshot;
use std::collections::BTreeSet;
use std::time::Instant;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ShadowInput {
    pub(crate) run_id: String,
    pub(crate) base: ContextWindow,
    pub(crate) existing_candidates: Vec<Candidate>,
    pub(crate) source_warning: Option<String>,
    pub(crate) allowed_scope_keys: BTreeSet<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ShadowStatus {
    BaselineError,
    WorldOmitted,
    Compared,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ShadowSummary {
    pub(crate) status: ShadowStatus,
    pub(crate) omission: Option<WorldOmission>,
    pub(crate) baseline_bytes: Option<usize>,
    pub(crate) proposed_bytes: Option<usize>,
    pub(crate) world_selected: bool,
    pub(crate) existing_selected_count: usize,
    pub(crate) world_bytes: usize,
    pub(crate) total_elapsed_ms: u64,
}

pub(crate) fn run_shadow(
    service: &WorldFrameService,
    source_request: WorldSourceRequest<'_>,
    input: &ShadowInput,
    clock: &dyn Fn() -> i64,
    scope: &ScopeSnapshot,
) -> ShadowSummary {
    let started = Instant::now();
    if input.run_id != source_request.frame_request.run_id {
        return finish(
            started,
            ShadowSummary {
                status: ShadowStatus::WorldOmitted,
                omission: Some(WorldOmission::ScopeDenied),
                baseline_bytes: None,
                proposed_bytes: None,
                world_selected: false,
                existing_selected_count: 0,
                world_bytes: 0,
                total_elapsed_ms: 0,
            },
        );
    }
    let baseline_input = broker_input(input, None);
    let baseline = match broker::compose(baseline_input) {
        Ok(envelope) => envelope,
        Err(_) => {
            return finish(
                started,
                ShadowSummary {
                    status: ShadowStatus::BaselineError,
                    omission: None,
                    baseline_bytes: None,
                    proposed_bytes: None,
                    world_selected: false,
                    existing_selected_count: 0,
                    world_bytes: 0,
                    total_elapsed_ms: 0,
                },
            );
        }
    };
    let baseline_bytes = baseline.health.projected_bytes;
    let existing_selected_count = baseline.selected.len();
    let outcome = prepare_candidate(service, source_request, scope);
    let ready = match outcome {
        WorldSourceOutcome::Omitted(omission) => {
            return omitted(
                started,
                Some(omission),
                baseline_bytes,
                existing_selected_count,
                0,
            );
        }
        WorldSourceOutcome::Ready(ready) => ready,
    };
    let world_bytes = ready.candidate().cost_bytes;
    match service.revalidate_frame(ready.prepared()) {
        Ok(validity) => {
            if let Some(omission) = omission_from_validity(validity) {
                return omitted(
                    started,
                    Some(omission),
                    baseline_bytes,
                    existing_selected_count,
                    world_bytes,
                );
            }
        }
        Err(error) => {
            return omitted(
                started,
                Some(omission_from_frame(error)),
                baseline_bytes,
                existing_selected_count,
                world_bytes,
            );
        }
    }
    if !within_ttl(ready.prepared().frame(), clock) {
        return omitted(
            started,
            Some(WorldOmission::Expired),
            baseline_bytes,
            existing_selected_count,
            world_bytes,
        );
    }
    if !scope_subset(&ready.candidate().scope_refs, &input.allowed_scope_keys) {
        return omitted(
            started,
            Some(WorldOmission::ScopeDenied),
            baseline_bytes,
            existing_selected_count,
            world_bytes,
        );
    }
    let proposed = match broker::compose(broker_input(input, Some(ready.candidate().clone()))) {
        Ok(envelope) => envelope,
        Err(_) => {
            return omitted(
                started,
                Some(WorldOmission::SourceError),
                baseline_bytes,
                existing_selected_count,
                world_bytes,
            );
        }
    };
    if displaced(&baseline, &proposed) {
        return finish(
            started,
            ShadowSummary {
                status: ShadowStatus::WorldOmitted,
                omission: Some(WorldOmission::WouldDisplace),
                baseline_bytes: Some(baseline_bytes),
                proposed_bytes: Some(baseline_bytes),
                world_selected: false,
                existing_selected_count,
                world_bytes,
                total_elapsed_ms: 0,
            },
        );
    }
    if proposed
        .omitted
        .iter()
        .any(|candidate| candidate.source_kind == WORLD_SHADOW_KIND)
    {
        return omitted(
            started,
            Some(WorldOmission::Budget),
            baseline_bytes,
            existing_selected_count,
            world_bytes,
        );
    }
    if !within_ttl(ready.prepared().frame(), clock) {
        return omitted(
            started,
            Some(WorldOmission::Expired),
            baseline_bytes,
            existing_selected_count,
            world_bytes,
        );
    }
    let world_selected = proposed
        .selected
        .iter()
        .any(|candidate| candidate.source_kind == WORLD_SHADOW_KIND);
    finish(
        started,
        ShadowSummary {
            status: ShadowStatus::Compared,
            omission: None,
            baseline_bytes: Some(baseline_bytes),
            proposed_bytes: Some(proposed.health.projected_bytes),
            world_selected,
            existing_selected_count,
            world_bytes,
            total_elapsed_ms: 0,
        },
    )
}

fn broker_input(input: &ShadowInput, extra: Option<Candidate>) -> BrokerInput {
    let mut candidates = input.existing_candidates.clone();
    if let Some(candidate) = extra {
        candidates.push(candidate);
    }
    BrokerInput {
        base: input.base.clone(),
        candidates,
        source_warning: input.source_warning.clone(),
        allowed_scope_keys: input.allowed_scope_keys.clone(),
    }
}

fn scope_subset(scope_refs: &[String], allowed: &BTreeSet<String>) -> bool {
    !scope_refs.is_empty() && scope_refs.iter().all(|scope| allowed.contains(scope))
}

fn displaced(baseline: &Envelope, proposed: &Envelope) -> bool {
    let proposed_ids: BTreeSet<_> = proposed.selected.iter().map(identity).collect();
    baseline
        .selected
        .iter()
        .any(|candidate| !proposed_ids.contains(&identity(candidate)))
}

fn identity(candidate: &Candidate) -> (&str, &str, u64, &str) {
    (
        candidate.source_kind.as_str(),
        candidate.source_id.as_str(),
        candidate.source_version,
        candidate.source_digest.as_str(),
    )
}

fn within_ttl(
    frame: &saaa_personal_state_core::world::runtime_frame::WorldFrame,
    clock: &dyn Fn() -> i64,
) -> bool {
    let now = clock();
    frame.captured_at_ms <= now && now < frame.expires_at_ms
}

fn omitted(
    started: Instant,
    omission: Option<WorldOmission>,
    baseline_bytes: usize,
    existing_selected_count: usize,
    world_bytes: usize,
) -> ShadowSummary {
    finish(
        started,
        ShadowSummary {
            status: ShadowStatus::WorldOmitted,
            omission,
            baseline_bytes: Some(baseline_bytes),
            proposed_bytes: None,
            world_selected: false,
            existing_selected_count,
            world_bytes,
            total_elapsed_ms: 0,
        },
    )
}

fn finish(started: Instant, mut summary: ShadowSummary) -> ShadowSummary {
    summary.total_elapsed_ms = started.elapsed().as_millis() as u64;
    summary
}
