//! WorldFrame service used by conversation Context Broker composition.
//! It composes a persisted `WorldSliceV2` with a live
//! current-state view of trusted runtimes, and can re-verify a prepared frame.
#![allow(dead_code)]

use super::query::WorldSeed;
use super::query_v2::IncludeFlags;
use super::runtime_capacity;
use super::runtime_coding;
use super::runtime_graph::{self, GraphOutcome};
use super::runtime_scope::{authorize_frame_request, AuthorizedFrame};
use crate::persistence::sqlite::SqliteReaders;
use rusqlite::Connection;
use saaa_personal_state_core::world::runtime_frame::{
    assemble_frame, compare_stamp, content_digest, effective_max_bytes, is_within_validity,
    normalize_runtime_refs, normalize_ttl, FrameAssembly, FrameError, FrameNotice, FrameNoticeCode,
    FrameStamp, FrameValidity, RuntimeKind, RuntimeRef, RuntimeUnit, WorldFrame,
};
use saaa_personal_state_core::world::slice_v2::WorldSliceV2;
use saaa_personal_state_core::world::traversal_v2::{CausalDirection, LimitsV2};
use saaa_personal_state_core::{AccessRequest, Classification, Purpose};
use std::sync::Arc;

fn frame_error_from_code(code: &str) -> FrameError {
    match code {
        "frame-invalid-input" => FrameError::InvalidInput,
        "frame-limit" => FrameError::Limit,
        "frame-scope-denied" => FrameError::ScopeDenied,
        "frame-changed" => FrameError::Changed,
        "frame-expired" => FrameError::Expired,
        "frame-owner-corrupt" => FrameError::OwnerCorrupt,
        "frame-budget-too-small" => FrameError::BudgetTooSmall,
        "frame-unsupported-evidence-contract" => FrameError::UnsupportedEvidenceContract,
        "runtime_unavailable" => FrameError::Unavailable,
        "runtime_unsupported_state" => FrameError::UnsupportedState,
        other => FrameError::Other(other.to_string()),
    }
}

#[derive(Debug, Clone)]
pub(crate) struct GraphRequest {
    pub(crate) seeds: Vec<WorldSeed>,
    pub(crate) causal_direction: CausalDirection,
    pub(crate) limits: LimitsV2,
    pub(crate) flags: IncludeFlags,
    pub(crate) explicit_question: bool,
}

pub(crate) struct FrameRequest<'a> {
    pub(crate) run_id: &'a str,
    pub(crate) project_scope: &'a str,
    pub(crate) access: AccessRequest<'a>,
    pub(crate) runtime_refs: Vec<RuntimeRef>,
    pub(crate) graph_request: Option<GraphRequest>,
    pub(crate) max_bytes: usize,
    pub(crate) ttl_ms: u64,
}

#[derive(Debug, Clone)]
pub(crate) struct OwnedAccess {
    pub(crate) principal: String,
    pub(crate) scope: String,
    pub(crate) task_request: Option<String>,
    pub(crate) purpose: Purpose,
    pub(crate) max_classification: Classification,
    pub(crate) policy_revision: u64,
    pub(crate) authorized: bool,
}

impl OwnedAccess {
    pub(crate) fn as_request(&self) -> AccessRequest<'_> {
        AccessRequest {
            principal: &self.principal,
            scope: &self.scope,
            task_request: self.task_request.as_deref(),
            purpose: self.purpose,
            max_classification: self.max_classification,
            policy_revision: self.policy_revision,
            authorized: self.authorized,
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct OwnedFrameRequest {
    pub(crate) run_id: String,
    pub(crate) project_scope: String,
    pub(crate) access: OwnedAccess,
    pub(crate) runtime_refs: Vec<RuntimeRef>,
    pub(crate) graph_request: Option<GraphRequest>,
    pub(crate) max_bytes: usize,
    pub(crate) ttl_ms: u64,
}

impl OwnedFrameRequest {
    fn from_borrowed(
        request: &FrameRequest<'_>,
        runtime_refs: Vec<RuntimeRef>,
        max_bytes: usize,
        ttl_ms: u64,
    ) -> Self {
        Self {
            run_id: request.run_id.to_string(),
            project_scope: request.project_scope.to_string(),
            access: OwnedAccess {
                principal: request.access.principal.to_string(),
                scope: request.access.scope.to_string(),
                task_request: request.access.task_request.map(str::to_string),
                purpose: request.access.purpose,
                max_classification: request.access.max_classification,
                policy_revision: request.access.policy_revision,
                authorized: request.access.authorized,
            },
            runtime_refs,
            graph_request: request.graph_request.clone(),
            max_bytes,
            ttl_ms,
        }
    }

    pub(crate) fn as_borrowed<'a>(&'a self, access: AccessRequest<'a>) -> FrameRequest<'a> {
        FrameRequest {
            run_id: &self.run_id,
            project_scope: &self.project_scope,
            access,
            runtime_refs: self.runtime_refs.clone(),
            graph_request: self.graph_request.clone(),
            max_bytes: self.max_bytes,
            ttl_ms: self.ttl_ms,
        }
    }

    /// A stable fingerprint of the whole normalized request. It is stored in
    /// `PreparedWorldFrame` so the request and the stamp it produced can be
    /// reconciled without exposing the request itself.
    pub(crate) fn fingerprint(&self) -> String {
        let graph = self.graph_request.as_ref().map(|graph| {
            let seeds: Vec<serde_json::Value> = graph
                .seeds
                .iter()
                .map(|seed| match seed {
                    WorldSeed::EntityId(id) => serde_json::json!(["id", id]),
                    WorldSeed::ExactName(name) => serde_json::json!(["name", name]),
                })
                .collect();
            serde_json::json!({
                "seeds": seeds,
                "direction": format!("{:?}", graph.causal_direction),
                "limits": [
                    graph.limits.causal_depth,
                    graph.limits.relevance_depth,
                    graph.limits.nodes,
                    graph.limits.edges,
                    graph.limits.paths,
                    graph.limits.fetch_rows,
                    graph.limits.scan_steps,
                ],
                "flags": [
                    graph.flags.causal,
                    graph.flags.goals,
                    graph.flags.correlations,
                    graph.flags.dependencies,
                ],
                "explicit_question": graph.explicit_question,
            })
        });
        let runtime_refs: Vec<serde_json::Value> = self
            .runtime_refs
            .iter()
            .map(|reference| serde_json::json!([format!("{:?}", reference.kind), reference.id]))
            .collect();
        let value = serde_json::json!({
            "run_id": self.run_id,
            "project_scope": self.project_scope,
            "principal": self.access.principal,
            "scope": self.access.scope,
            "task_request": self.access.task_request,
            "purpose": self.access.purpose,
            "max_classification": self.access.max_classification,
            "policy_revision": self.access.policy_revision,
            "authorized": self.access.authorized,
            "runtime_refs": runtime_refs,
            "graph": graph,
            "max_bytes": self.max_bytes,
            "ttl_ms": self.ttl_ms,
        });
        let text = serde_json::to_string(&value).unwrap_or_default();
        saaa_personal_state_core::world::runtime_frame::hex_sha256(text.as_bytes())
    }
}

#[derive(Debug, Clone)]
pub(crate) struct PreparedWorldFrame {
    frame: WorldFrame,
    request: OwnedFrameRequest,
    request_fingerprint: String,
    stamp: FrameStamp,
    instance_id: String,
}

impl PreparedWorldFrame {
    pub(crate) fn frame(&self) -> &WorldFrame {
        &self.frame
    }

    #[cfg(test)]
    pub(crate) fn stamp(&self) -> &FrameStamp {
        &self.stamp
    }

    #[cfg(test)]
    pub(crate) fn request(&self) -> &OwnedFrameRequest {
        &self.request
    }

    #[cfg(test)]
    pub(crate) fn instance_id(&self) -> &str {
        &self.instance_id
    }
}

struct DbOutcome {
    runtime: Vec<RuntimeUnit>,
    runtime_notices: Vec<FrameNotice>,
    graph: Option<WorldSliceV2>,
    graph_notice: Option<FrameNoticeCode>,
}

fn read_db(
    c: &Connection,
    request: &FrameRequest<'_>,
    authorized: &AuthorizedFrame,
    now: i64,
) -> Result<DbOutcome, FrameError> {
    let mut runtime = Vec::new();
    let mut notices = Vec::new();
    for target in &authorized.targets {
        match target.reference.kind {
            RuntimeKind::CodingJob => {
                let read = runtime_coding::read_view(c, authorized, &target.reference)?;
                match read.unit {
                    Some(unit) => runtime.push(unit),
                    None => notices.push(FrameNotice::for_ref(
                        read.notice.unwrap_or(FrameNoticeCode::RuntimeUnavailable),
                        target.reference.clone(),
                    )),
                }
            }
        }
    }

    let mut graph = None;
    let mut graph_notice = None;
    if let Some(graph_request) = &request.graph_request {
        // The capacity preflight exists only to bound `store::load`; a request
        // without a graph never reads the ledger and skips it entirely.
        let capacity = runtime_capacity::check_frame_capacity(c, &authorized.project_scope)?;
        if capacity.exceeded {
            graph_notice = Some(FrameNoticeCode::WorldCapacityOmitted);
        } else {
            match runtime_graph::load_graph(
                c,
                request,
                authorized,
                now,
                runtime.len(),
                graph_request,
            )? {
                GraphOutcome::Slice(slice) => graph = Some(slice),
                GraphOutcome::Notice(code) => graph_notice = Some(code),
            }
        }
    } else {
        graph_notice = Some(FrameNoticeCode::GraphNotRequested);
    }
    Ok(DbOutcome {
        runtime,
        runtime_notices: notices,
        graph,
        graph_notice,
    })
}

pub(crate) struct WorldFrameService {
    readers: SqliteReaders,
    clock: Arc<dyn Fn() -> i64 + Send + Sync>,
    instance_id: String,
    situation: Option<Arc<crate::situation::SituationRuntime>>,
}

impl WorldFrameService {
    pub(crate) fn new(readers: SqliteReaders, clock: Arc<dyn Fn() -> i64 + Send + Sync>) -> Self {
        Self {
            readers,
            clock,
            instance_id: crate::new_id("world_frame"),
            situation: None,
        }
    }

    pub(crate) fn with_sources(
        mut self,
        situation: Arc<crate::situation::SituationRuntime>,
    ) -> Self {
        self.situation = Some(situation);
        self
    }
    pub(crate) fn sources_enabled(&self) -> bool {
        self.situation.is_some()
    }

    fn run<T>(
        &self,
        operation: impl FnOnce(&Connection) -> Result<T, FrameError>,
    ) -> Result<T, FrameError> {
        self.readers
            .read(|connection| operation(connection).map_err(|error| error.code().to_string()))
            .map_err(|code| frame_error_from_code(&code))
    }

    fn build(&self, owned: OwnedFrameRequest) -> Result<PreparedWorldFrame, FrameError> {
        let access = owned.access.as_request();
        let request = owned.as_borrowed(access);
        let now = (self.clock)();
        let expires = now.saturating_add(owned.ttl_ms as i64);
        let first = self.run(|c| authorize_frame_request(c, &request))?;
        let user_scope = first
            .scope
            .allowed_scope_keys
            .iter()
            .find(|key| key.starts_with("user:"));
        let before = match (&self.situation, user_scope) {
            (Some(situation), Some(scope)) => Some(
                situation
                    .world_snapshot(scope, now)
                    .map_err(|_| FrameError::Unavailable)?,
            ),
            _ => None,
        };
        let (outcome, mut sources) = self.run(|c| {
            let authorized = authorize_frame_request(c, &request)?;
            if authorized.scope_digest != first.scope_digest
                || authorized.ledger_revision != first.ledger_revision
                || authorized.input_epoch != first.input_epoch
                || authorized.policy_revision != first.policy_revision
            {
                return Err(FrameError::Changed);
            }
            let sources = if self.sources_enabled() {
                super::runtime_sources::read(c, &authorized, now)?
            } else {
                Vec::new()
            };
            Ok((read_db(c, &request, &authorized, now)?, sources))
        })?;
        if let (Some(before), Some(situation), Some(user_scope)) =
            (before, &self.situation, user_scope)
        {
            let after = situation
                .world_snapshot(user_scope, (self.clock)())
                .map_err(|_| FrameError::Unavailable)?;
            if before.digest != after.digest || before.version != after.version {
                return Err(FrameError::Changed);
            }
            use saaa_personal_state_core::world::frame_sources::*;
            sources.push(WorldSourceGroup {
                kind: WorldSourceKind::Situation,
                availability: WorldSourceAvailability::Available,
                entries: vec![after],
                omission_reason: None,
            });
        }

        let runtime = outcome.runtime;
        let mut notices = outcome.runtime_notices;
        let mut graph_capacity_omitted = false;
        if let Some(code) = outcome.graph_notice {
            notices.push(FrameNotice::global(code));
            if code == FrameNoticeCode::WorldCapacityOmitted {
                graph_capacity_omitted = true;
            }
        }
        // Reserve the complete source/scope envelope before the bounded graph/runtime assembler.
        let original = WorldFrame::empty(&owned.run_id, &owned.project_scope, now, expires);
        let mut enriched = original.clone();
        enriched.scope = first.scope.clone();
        enriched.project_scope = first.scope.focus_scope_key.clone().unwrap_or_default();
        enriched.sources = sources;
        let overhead = enriched
            .encoded_len()?
            .saturating_sub(original.encoded_len()?);
        let remaining = owned
            .max_bytes
            .checked_sub(overhead)
            .ok_or(FrameError::BudgetTooSmall)?;
        let mut frame = assemble_frame(FrameAssembly {
            run_id: &owned.run_id,
            project_scope: &owned.project_scope,
            captured_at_ms: now,
            expires_at_ms: expires,
            max_bytes: remaining,
            graph: outcome.graph,
            runtime,
            notices,
        })?;
        frame.scope = enriched.scope;
        frame.project_scope = enriched.project_scope;
        frame.sources = enriched.sources;
        if frame.encoded_len()? > owned.max_bytes {
            return Err(FrameError::BudgetTooSmall);
        }
        if graph_capacity_omitted {
            frame.truncated = true;
        }
        let stamp = FrameStamp {
            ledger_revision: first.ledger_revision,
            input_epoch: first.input_epoch,
            policy_revision: first.policy_revision,
            scope_digest: first.scope_digest,
            owner_digests: frame
                .runtime
                .iter()
                .map(|view| (view.reference.clone(), view.owner_digest.clone()))
                .collect(),
            content_digest: content_digest(&frame)?,
        };
        let request_fingerprint = owned.fingerprint();
        Ok(PreparedWorldFrame {
            frame,
            request: owned,
            request_fingerprint,
            stamp,
            instance_id: self.instance_id.clone(),
        })
    }

    /// Result acceptance ignores elapsed TTL, but never ignores changed dependencies.
    pub(crate) fn validate_result(
        &self,
        prior: &PreparedWorldFrame,
    ) -> Result<FrameValidity, FrameError> {
        let current = self.build(prior.request.clone())?;
        Ok(compare_stamp(&prior.stamp, &current.stamp))
    }

    /// New generation only: creates a fresh stamp; never changes the old frame's TTL.
    pub(crate) fn refresh(
        &self,
        prior: &PreparedWorldFrame,
    ) -> Result<PreparedWorldFrame, FrameError> {
        let access = prior.request.access.as_request();
        self.prepare_frame(prior.request.as_borrowed(access))
    }

    pub(crate) fn prepare_frame(
        &self,
        request: FrameRequest<'_>,
    ) -> Result<PreparedWorldFrame, FrameError> {
        let ttl_ms = normalize_ttl(request.ttl_ms)?;
        let max_bytes = effective_max_bytes(request.max_bytes);
        let runtime_refs = normalize_runtime_refs(&request.runtime_refs)?;
        let owned = OwnedFrameRequest::from_borrowed(&request, runtime_refs, max_bytes, ttl_ms);
        let prepared = match self.build(owned.clone()) {
            Err(FrameError::Changed) => self.build(owned)?,
            result => result?,
        };
        if !is_within_validity(
            prepared.frame.captured_at_ms,
            prepared.frame.expires_at_ms,
            (self.clock)(),
        ) {
            return Err(FrameError::Expired);
        }
        Ok(prepared)
    }

    pub(crate) fn revalidate_frame(
        &self,
        prepared: &PreparedWorldFrame,
    ) -> Result<FrameValidity, FrameError> {
        if prepared.instance_id != self.instance_id {
            return Ok(FrameValidity::Expired);
        }
        // The stored request and stamp must belong together.
        if prepared.request_fingerprint != prepared.request.fingerprint() {
            return Ok(FrameValidity::Unavailable);
        }
        let now = (self.clock)();
        if !is_within_validity(
            prepared.frame.captured_at_ms,
            prepared.frame.expires_at_ms,
            now,
        ) {
            return Ok(FrameValidity::Expired);
        }
        // Cheap header re-check first: ledger / epoch / policy / scope changes
        // are rejected without rebuilding the graph (and without store::load).
        let access = prepared.request.access.as_request();
        let borrowed = prepared.request.as_borrowed(access);
        let header = match self.run(|c| authorize_frame_request(c, &borrowed)) {
            Ok(header) => header,
            Err(FrameError::ScopeDenied) => return Ok(FrameValidity::ScopeDenied),
            Err(error) => return Err(error),
        };
        if header.scope_digest != prepared.stamp.scope_digest
            || header.ledger_revision != prepared.stamp.ledger_revision
            || header.input_epoch != prepared.stamp.input_epoch
            || header.policy_revision != prepared.stamp.policy_revision
        {
            return Ok(FrameValidity::Changed);
        }
        let rebuilt = match self.build(prepared.request.clone()) {
            Ok(rebuilt) => rebuilt,
            Err(FrameError::ScopeDenied) => return Ok(FrameValidity::ScopeDenied),
            Err(FrameError::Unavailable) => return Ok(FrameValidity::Unavailable),
            Err(FrameError::Changed) => return Ok(FrameValidity::Changed),
            Err(FrameError::Expired) => return Ok(FrameValidity::Expired),
            Err(error) => return Err(error),
        };
        // Rebuilding may cross the original deadline; its fresh timestamps must not renew it.
        if !is_within_validity(
            prepared.frame.captured_at_ms,
            prepared.frame.expires_at_ms,
            (self.clock)(),
        ) {
            return Ok(FrameValidity::Expired);
        }
        Ok(compare_stamp(&prepared.stamp, &rebuilt.stamp))
    }
}

#[path = "runtime_frame_receipt.rs"]
mod receipt;
