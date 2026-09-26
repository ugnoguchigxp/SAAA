//! Shared World frame helpers used by non-conversation provider paths.
use super::source::WorldOmission;
use crate::runtime::context::scope::ScopeSnapshot;
use saaa_personal_state_core::world::runtime_frame::{RuntimeKind, RuntimeRef, MAX_RUNTIME_REFS};
use std::collections::BTreeSet;

pub(crate) use super::dispatch::for_record;
pub(crate) use super::live::{observe_receipt, WorldBlocks, WorldLive, WorldReceipt};

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
        let Some(id) = (item.kind == "task")
            .then(|| item.key.strip_prefix("task:"))
            .flatten()
        else {
            continue;
        };
        let reference = RuntimeRef {
            kind: RuntimeKind::CodingJob,
            id: id.to_string(),
        };
        if seen.insert((reference.kind, reference.id.clone())) {
            refs.push(reference);
        }
        if refs.len() == MAX_RUNTIME_REFS {
            break;
        }
    }
    refs
}
