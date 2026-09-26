//! Shared final-frame operations; receipt identities follow the actual refreshed body.
use super::{source::WORLD_KIND, turn::WorldLive};
use crate::runtime::context::source::Candidate;
use serde_json::Value;
pub(crate) fn selected(
    selected: &[Candidate],
    world: Option<&WorldLive>,
    include: bool,
) -> Vec<Candidate> {
    let mut result = selected.to_vec();
    if include {
        if let Some(current) = world.and_then(WorldLive::current_candidate) {
            result.retain(|c| c.source_kind != WORLD_KIND);
            result.push(current);
        }
    }
    result
}
pub(crate) fn refresh_json(
    messages: &mut [Value],
    world: Option<&WorldLive>,
) -> Result<(), String> {
    let Some(world) = world else {
        return Ok(());
    };
    let Some((old, new)) = world.refresh_blocks()? else {
        return Ok(());
    };
    let mut count = 0;
    for message in messages {
        if message["role"] == "assistant" && message["content"].as_str() == Some(&old.with_world) {
            message["content"] = Value::String(new.with_world.clone());
            count += 1;
        }
    }
    if count != 1 {
        return Err("world-wire-provenance-mismatch".into());
    }
    Ok(())
}

/// The candidates the record should contain for one request. The World candidate is only kept when
/// the caller decided to include it in the sent body; a World-free history always drops it.
pub(crate) fn for_record<'a>(
    selected: &'a [Candidate],
    omitted: &'a [Candidate],
    include_world: bool,
) -> (Vec<&'a Candidate>, Vec<&'a Candidate>) {
    let mut kept: Vec<&Candidate> = selected
        .iter()
        .filter(|candidate| candidate.source_kind != WORLD_KIND)
        .collect();
    let mut omitted: Vec<&Candidate> = omitted.iter().collect();
    if !include_world {
        omitted.extend(selected.iter().filter(|c| c.source_kind == WORLD_KIND));
    }
    if include_world {
        if let Some(candidate) = selected
            .iter()
            .find(|candidate| candidate.source_kind == WORLD_KIND)
        {
            kept.push(candidate);
        }
    }
    (kept, omitted)
}
