//! Trusted WorldFrame → one untrusted Broker candidate (S1/S3/S4).
use super::super::source::{Candidate, Requirement};
use super::render::{render_world_frame, render_world_frame_explicit, RenderOmission};
use crate::memory::personal_state::world::query::WorldSeed;
use crate::memory::personal_state::world::runtime_frame::{
    FrameRequest, PreparedWorldFrame, WorldFrameService,
};
use crate::runtime::context::scope::ScopeSnapshot;
use saaa_personal_state_core::world::runtime_frame::{FrameError, FrameValidity, MAX_RUNTIME_REFS};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;

pub(crate) const WORLD_SHADOW_KIND: &str = "world-model-shadow";
pub(crate) const WORLD_KIND: &str = "world-model";
const MAX_GRAPH_SEEDS: usize = 4;

#[cfg(test)]
thread_local! {
    static PREPARE_CALLS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WorldOmission {
    NoExplicitProject,
    AmbiguousProject,
    ScopeDenied,
    EmptyRequest,
    EmptyFrame,
    InvalidInput,
    Limit,
    Expired,
    Changed,
    Unavailable,
    Budget,
    WouldDisplace,
    SourceError,
}

impl WorldOmission {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::NoExplicitProject => "no_explicit_project",
            Self::AmbiguousProject => "ambiguous_project",
            Self::ScopeDenied => "scope_denied",
            Self::EmptyRequest => "empty_request",
            Self::EmptyFrame => "empty_frame",
            Self::InvalidInput => "invalid_input",
            Self::Limit => "limit",
            Self::Expired => "expired",
            Self::Changed => "changed",
            Self::Unavailable => "unavailable",
            Self::Budget => "budget",
            Self::WouldDisplace => "would_displace",
            Self::SourceError => "source_error",
        }
    }
}

pub(crate) struct WorldSourceRequest<'a> {
    pub(crate) frame_request: FrameRequest<'a>,
}

/// C3: the only distinction the shared inspection helper needs. `EntityIdsOnly` is the existing
/// shadow/internal entry; `ExplicitQuestionName` additionally accepts one `ExactName` seed and can
/// only be reached from the production question entry built from a parsed `GraphQuestion`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SeedPolicy {
    EntityIdsOnly,
    ExplicitQuestionName,
}

pub(crate) struct PreparedWorldCandidate {
    prepared: PreparedWorldFrame,
    candidate: Candidate,
}

impl PreparedWorldCandidate {
    pub(crate) fn candidate(&self) -> &Candidate {
        &self.candidate
    }

    pub(crate) fn prepared(&self) -> &PreparedWorldFrame {
        &self.prepared
    }

    pub(crate) fn into_prepared(self) -> PreparedWorldFrame {
        self.prepared
    }
}

pub(crate) fn with_kind(
    ready: Box<PreparedWorldCandidate>,
    kind: &str,
) -> Box<PreparedWorldCandidate> {
    let candidate = frame_candidate(ready.prepared.frame(), &ready.candidate.content, kind);
    Box::new(PreparedWorldCandidate {
        prepared: ready.prepared,
        candidate,
    })
}

pub(crate) enum WorldSourceOutcome {
    Ready(Box<PreparedWorldCandidate>),
    Omitted(WorldOmission),
}

pub(crate) fn inspect_request(
    request: &WorldSourceRequest<'_>,
    scope: &ScopeSnapshot,
) -> Result<(), WorldOmission> {
    inspect_request_with(request, scope, SeedPolicy::EntityIdsOnly)
}

fn inspect_request_with(
    request: &WorldSourceRequest<'_>,
    scope: &ScopeSnapshot,
    policy: SeedPolicy,
) -> Result<(), WorldOmission> {
    if scope.status != "resolved" {
        return Err(WorldOmission::ScopeDenied);
    }
    inspect_project(request.frame_request.project_scope, scope)?;
    inspect_graph_and_refs(request, policy)?;
    Ok(())
}

fn inspect_project(project_scope: &str, scope: &ScopeSnapshot) -> Result<(), WorldOmission> {
    if project_scope.is_empty() {
        return Err(WorldOmission::NoExplicitProject);
    }
    if project_scope.contains(',') || project_scope.contains('\n') || project_scope.contains(';') {
        return Err(WorldOmission::AmbiguousProject);
    }
    if project_scope.starts_with("user:")
        && scope.is_user_only()
        && scope
            .scopes
            .iter()
            .any(|item| item.key == project_scope && item.kind == "user")
    {
        return Ok(());
    }
    let projects: BTreeSet<&str> = scope
        .scopes
        .iter()
        .filter(|item| {
            item.kind == "project" && matches!(item.relation.as_str(), "focus" | "parent")
        })
        .map(|item| item.key.as_str())
        .collect();
    if !projects.contains(project_scope) {
        return Err(WorldOmission::ScopeDenied);
    }
    Ok(())
}

fn inspect_graph_and_refs(
    request: &WorldSourceRequest<'_>,
    policy: SeedPolicy,
) -> Result<(), WorldOmission> {
    if request.frame_request.runtime_refs.len() > MAX_RUNTIME_REFS {
        return Err(WorldOmission::Limit);
    }
    let seeds_empty = match &request.frame_request.graph_request {
        None => true,
        Some(graph) => {
            let mut ids = BTreeSet::new();
            for seed in &graph.seeds {
                match seed {
                    WorldSeed::EntityId(id) => {
                        if id.is_empty() {
                            return Err(WorldOmission::InvalidInput);
                        }
                        ids.insert(id.as_str());
                    }
                    WorldSeed::ExactName(name) => {
                        if policy != SeedPolicy::ExplicitQuestionName {
                            return Err(WorldOmission::InvalidInput);
                        }
                        if name.is_empty() || name.len() > 160 || name.chars().any(char::is_control)
                        {
                            return Err(WorldOmission::InvalidInput);
                        }
                        ids.insert(name.as_str());
                    }
                }
            }
            if ids.len() > MAX_GRAPH_SEEDS {
                return Err(WorldOmission::Limit);
            }
            ids.is_empty()
        }
    };
    if seeds_empty && request.frame_request.runtime_refs.is_empty() {
        return Err(WorldOmission::EmptyRequest);
    }
    Ok(())
}

pub(crate) fn prepare_candidate(
    service: &WorldFrameService,
    request: WorldSourceRequest<'_>,
    scope: &ScopeSnapshot,
) -> WorldSourceOutcome {
    prepare_candidate_with(service, request, scope, SeedPolicy::EntityIdsOnly)
}

/// C3/C5: the production explicit-question entry. It accepts one `ExactName` seed and renders an
/// otherwise-empty frame that carries a resolution/freshness constraint.
pub(crate) fn prepare_explicit_question_candidate(
    service: &WorldFrameService,
    request: WorldSourceRequest<'_>,
    scope: &ScopeSnapshot,
) -> WorldSourceOutcome {
    prepare_candidate_with(service, request, scope, SeedPolicy::ExplicitQuestionName)
}

fn prepare_candidate_with(
    service: &WorldFrameService,
    request: WorldSourceRequest<'_>,
    scope: &ScopeSnapshot,
    policy: SeedPolicy,
) -> WorldSourceOutcome {
    if let Err(omission) = inspect_request_with(&request, scope, policy) {
        if !(omission == WorldOmission::EmptyRequest && service.sources_enabled()) {
            return WorldSourceOutcome::Omitted(omission);
        }
    }
    let explicit_rendering = policy == SeedPolicy::ExplicitQuestionName;
    let mut request = request;
    if let Some(graph) = request.frame_request.graph_request.as_mut() {
        if graph.seeds.is_empty() {
            request.frame_request.graph_request = None;
        }
    }
    #[cfg(test)]
    PREPARE_CALLS.with(|count| count.set(count.get() + 1));
    let prepared = match service.prepare_frame(request.frame_request) {
        Ok(prepared) => prepared,
        Err(error) => return WorldSourceOutcome::Omitted(omission_from_frame(error)),
    };
    let content = match if explicit_rendering {
        render_world_frame_explicit(prepared.frame())
    } else {
        render_world_frame(prepared.frame())
    } {
        Ok(content) => content,
        Err(RenderOmission::EmptyFrame) => {
            return WorldSourceOutcome::Omitted(WorldOmission::EmptyFrame);
        }
        Err(RenderOmission::Budget) => {
            return WorldSourceOutcome::Omitted(WorldOmission::Budget);
        }
        Err(RenderOmission::Encode) => {
            return WorldSourceOutcome::Omitted(WorldOmission::SourceError);
        }
    };
    let candidate = frame_candidate(prepared.frame(), &content, WORLD_SHADOW_KIND);
    WorldSourceOutcome::Ready(Box::new(PreparedWorldCandidate {
        prepared,
        candidate,
    }))
}

pub(crate) fn frame_candidate(
    frame: &saaa_personal_state_core::world::runtime_frame::WorldFrame,
    content: &str,
    kind: &str,
) -> Candidate {
    let digest = format!("{:x}", Sha256::digest(content.as_bytes()));
    let run_id = frame.run_id.as_str();
    let mut scope_refs: BTreeSet<String> = frame.scope.allowed_scope_keys.iter().cloned().collect();
    for view in &frame.runtime {
        scope_refs.insert(view.scope_key.clone());
    }
    Candidate::untrusted(
        format!("world-frame:{run_id}:{digest}"),
        kind,
        scope_refs.into_iter().collect(),
        Requirement::May,
        format!("world-frame:{run_id}:{digest}"),
        1,
        0,
        content.to_string(),
    )
}

pub(crate) fn omission_from_frame(error: FrameError) -> WorldOmission {
    match error {
        FrameError::InvalidInput => WorldOmission::InvalidInput,
        FrameError::Limit => WorldOmission::Limit,
        FrameError::ScopeDenied => WorldOmission::ScopeDenied,
        FrameError::Changed => WorldOmission::Changed,
        FrameError::Expired => WorldOmission::Expired,
        FrameError::Unavailable => WorldOmission::Unavailable,
        FrameError::BudgetTooSmall => WorldOmission::Budget,
        FrameError::OwnerCorrupt
        | FrameError::UnsupportedEvidenceContract
        | FrameError::UnsupportedState
        | FrameError::Other(_) => WorldOmission::SourceError,
    }
}

pub(crate) fn omission_from_validity(validity: FrameValidity) -> Option<WorldOmission> {
    match validity {
        FrameValidity::Current => None,
        FrameValidity::Changed => Some(WorldOmission::Changed),
        FrameValidity::Expired => Some(WorldOmission::Expired),
        FrameValidity::ScopeDenied => Some(WorldOmission::ScopeDenied),
        FrameValidity::Unavailable => Some(WorldOmission::Unavailable),
    }
}

pub(crate) fn reject_dispatch(selected: &[Candidate], omitted: &[Candidate]) -> Result<(), String> {
    if selected
        .iter()
        .chain(omitted.iter())
        .any(|candidate| candidate.source_kind == WORLD_SHADOW_KIND)
    {
        return Err("world-shadow-not-dispatchable".into());
    }
    Ok(())
}

#[cfg(test)]
pub(crate) fn reset_prepare_calls() {
    PREPARE_CALLS.with(|count| count.set(0));
}

#[cfg(test)]
pub(crate) fn prepare_calls() -> usize {
    PREPARE_CALLS.with(|count| count.get())
}
