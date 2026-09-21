//! Production WorldFrame compose for conversation.respond (M3B).
use super::super::broker::{self, BrokerInput, Envelope};
use super::super::source::Candidate;
use super::question_input;
use super::source::{
    omission_from_frame, omission_from_validity, prepare_candidate,
    prepare_explicit_question_candidate, with_kind, WorldOmission, WorldSourceOutcome,
    WorldSourceRequest, WORLD_KIND,
};
use crate::memory::context_window::ContextWindow;
use crate::memory::personal_state::world::runtime_frame::{
    FrameRequest, GraphRequest, WorldFrameService,
};
use crate::runtime::context::scope::ScopeSnapshot;
use crate::AppState;
use saaa_personal_state_core::world::runtime_frame::{RuntimeKind, RuntimeRef, MAX_RUNTIME_REFS};
use saaa_personal_state_core::{AccessRequest, Classification, Purpose};
use std::collections::BTreeSet;
use std::sync::Arc;

pub(crate) use super::live::{for_record, observe_receipt, WorldBlocks, WorldLive, WorldReceipt};

pub(crate) struct TurnCompose {
    pub(crate) envelope: Envelope,
    pub(crate) world: Option<WorldLive>,
    pub(crate) compose_count: usize,
    pub(crate) omission: Option<WorldOmission>,
}

pub(crate) fn compose_for_app(
    state: &AppState,
    run_id: &str,
    scope: &ScopeSnapshot,
    base: ContextWindow,
    existing: Vec<Candidate>,
    allowed: BTreeSet<String>,
) -> Result<TurnCompose, String> {
    if !crate::memory::control_plane::memory_enabled() {
        return compose_parts(
            false, None, "", 0, None, run_id, scope, base, existing, allowed,
        );
    }
    let Some((principal, policy_revision)) = personal_access(&state.sqlite_readers) else {
        return compose_parts(
            true, None, "", 0, None, run_id, scope, base, existing, allowed,
        );
    };
    let service = Arc::new(WorldFrameService::new(
        state.sqlite_readers.clone(),
        Arc::new(crate::memory::personal_state::now),
    ));
    // C2: the graph question is resolved from the saved current input for this run, never from a
    // caller-supplied or provider-supplied string.
    let graph_request = question_input::read(state, run_id).graph_request();
    compose_parts(
        true,
        Some(service),
        &principal,
        policy_revision,
        graph_request,
        run_id,
        scope,
        base,
        existing,
        allowed,
    )
}

fn personal_access(readers: &crate::persistence::sqlite::SqliteReaders) -> Option<(String, u64)> {
    readers
        .read(|connection| {
            connection
                .query_row(
                    "SELECT principal,policy_revision FROM personal_scope WHERE id='primary'",
                    [],
                    |row| Ok((row.get::<_, String>(0)?, row.get::<_, u64>(1)?)),
                )
                .map_err(crate::database_error)
        })
        .ok()
        .filter(|(principal, _)| !principal.is_empty())
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn compose_parts(
    memory_on: bool,
    service: Option<Arc<WorldFrameService>>,
    principal: &str,
    policy_revision: u64,
    graph_request: Option<GraphRequest>,
    run_id: &str,
    scope: &ScopeSnapshot,
    base: ContextWindow,
    existing: Vec<Candidate>,
    allowed: BTreeSet<String>,
) -> Result<TurnCompose, String> {
    let baseline = broker::compose(BrokerInput {
        base: base.clone(),
        candidates: existing.clone(),
        source_warning: None,
        allowed_scope_keys: allowed.clone(),
    })?;
    if !memory_on {
        return Ok(done(baseline, None, 1, None));
    }
    let Some(service) = service else {
        return Ok(done(baseline, None, 1, None));
    };
    if principal.is_empty() {
        return Ok(done(baseline, None, 1, Some(WorldOmission::ScopeDenied)));
    }
    let project = match explicit_project(scope) {
        Ok(project) => project,
        Err(omission) => return Ok(done(baseline, None, 1, Some(omission))),
    };
    let refs = runtime_refs(scope);
    let access = AccessRequest {
        principal,
        scope: "primary",
        task_request: Some(&project),
        purpose: Purpose::Reasoning,
        max_classification: Classification::Confidential,
        policy_revision,
        authorized: true,
    };
    let outcome = prepare_live(
        service.as_ref(),
        FrameRequest {
            run_id,
            project_scope: &project,
            access,
            runtime_refs: refs,
            graph_request,
            max_bytes: 8_192,
            ttl_ms: 1_000,
        },
        scope,
    );
    let ready = match outcome {
        WorldSourceOutcome::Omitted(omission) => {
            return Ok(done(baseline, None, 1, Some(omission)));
        }
        WorldSourceOutcome::Ready(ready) => ready,
    };
    match service.revalidate_frame(ready.prepared()) {
        Ok(validity) => {
            if let Some(omission) = omission_from_validity(validity) {
                return Ok(done(baseline, None, 1, Some(omission)));
            }
        }
        Err(error) => {
            return Ok(done(baseline, None, 1, Some(omission_from_frame(error))));
        }
    }
    if !scope_subset(&ready.candidate().scope_refs, &allowed) {
        return Ok(done(baseline, None, 1, Some(WorldOmission::ScopeDenied)));
    }
    let mut with_world = existing;
    with_world.push(ready.candidate().clone());
    let proposed = broker::compose(BrokerInput {
        base,
        candidates: with_world,
        source_warning: None,
        allowed_scope_keys: allowed,
    })?;
    if displaced(&baseline, &proposed) {
        return Ok(done(baseline, None, 2, Some(WorldOmission::WouldDisplace)));
    }
    if proposed
        .omitted
        .iter()
        .any(|candidate| candidate.source_kind == WORLD_KIND)
    {
        return Ok(done(baseline, None, 2, Some(WorldOmission::Budget)));
    }
    let selected = proposed
        .selected
        .iter()
        .any(|candidate| candidate.source_kind == WORLD_KIND);
    let world = if selected {
        let with_world = proposed.combined_block.clone();
        let without_world = baseline.combined_block.clone();
        Some(WorldLive::live(
            service,
            ready.into_prepared(),
            with_world.map(|with_world| WorldBlocks {
                with_world,
                without_world,
            }),
        ))
    } else {
        None
    };
    Ok(done(proposed, world, 2, None))
}

fn prepare_live(
    service: &WorldFrameService,
    request: FrameRequest<'_>,
    scope: &ScopeSnapshot,
) -> WorldSourceOutcome {
    let explicit_question = request
        .graph_request
        .as_ref()
        .is_some_and(|graph| graph.explicit_question);
    let prepared = if explicit_question {
        prepare_explicit_question_candidate(
            service,
            WorldSourceRequest {
                frame_request: request,
            },
            scope,
        )
    } else {
        prepare_candidate(
            service,
            WorldSourceRequest {
                frame_request: request,
            },
            scope,
        )
    };
    match prepared {
        WorldSourceOutcome::Omitted(omission) => WorldSourceOutcome::Omitted(omission),
        WorldSourceOutcome::Ready(ready) => WorldSourceOutcome::Ready(with_kind(ready, WORLD_KIND)),
    }
}

pub(crate) fn explicit_project(scope: &ScopeSnapshot) -> Result<String, WorldOmission> {
    let mut keys: Vec<&str> = scope
        .scopes
        .iter()
        .filter(|item| {
            item.kind == "project" && matches!(item.relation.as_str(), "focus" | "parent")
        })
        .map(|item| item.key.as_str())
        .collect();
    keys.sort_unstable();
    keys.dedup();
    match keys.as_slice() {
        [] => Err(WorldOmission::NoExplicitProject),
        [key] => Ok((*key).to_string()),
        _ => Err(WorldOmission::AmbiguousProject),
    }
}

pub(crate) fn runtime_refs(scope: &ScopeSnapshot) -> Vec<RuntimeRef> {
    let mut refs = Vec::new();
    let mut seen = BTreeSet::new();
    for item in &scope.scopes {
        if !matches!(item.relation.as_str(), "current" | "focus") {
            continue;
        }
        let reference = match item.kind.as_str() {
            "task" => item.key.strip_prefix("task:").map(|id| RuntimeRef {
                kind: RuntimeKind::CodingJob,
                id: id.to_string(),
            }),
            _ => None,
        };
        let Some(reference) = reference else {
            continue;
        };
        if !seen.insert((reference.kind, reference.id.clone())) {
            continue;
        }
        refs.push(reference);
        if refs.len() == MAX_RUNTIME_REFS {
            break;
        }
    }
    refs
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

fn done(
    envelope: Envelope,
    world: Option<WorldLive>,
    compose_count: usize,
    omission: Option<WorldOmission>,
) -> TurnCompose {
    TurnCompose {
        envelope,
        world,
        compose_count,
        omission,
    }
}
