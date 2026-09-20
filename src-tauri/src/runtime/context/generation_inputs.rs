use super::{generation::GenerationHandle, source::Candidate, world::turn::WorldLive};
use serde_json::Value;
use sha2::{Digest, Sha256};

pub(crate) fn record(
    generation: &GenerationHandle,
    health: &str,
    selected: &[Candidate],
    omitted: &[Candidate],
    tools: &[Value],
    world: Option<&WorldLive>,
) -> Result<(), String> {
    super::world::source::reject_dispatch(selected, omitted)?;
    let (selected, omitted) = super::world::turn::for_record(selected, omitted, world, generation);
    generation.set_health(health)?;
    for (candidate, included, reason) in selected
        .iter()
        .map(|candidate| (*candidate, true, None))
        .chain(
            omitted
                .iter()
                .map(|candidate| (*candidate, false, Some("budget-or-policy"))),
        )
    {
        generation.add_input(
            &candidate.source_kind,
            &candidate.source_id,
            candidate.source_version,
            &candidate.source_digest,
            candidate.requirement.as_str(),
            candidate.placement.as_str(),
            included,
            reason,
        )?;
    }
    for tool in tools {
        let encoded = serde_json::to_vec(tool).map_err(|_| "Tool definition is invalid")?;
        let name = tool["function"]["name"]
            .as_str()
            .ok_or("Tool definition has no function name")?;
        generation.add_input(
            "static-tool-offer",
            name,
            1,
            &format!("{:x}", Sha256::digest(&encoded)),
            "should",
            "tool-schema",
            true,
            None,
        )?;
    }
    generation.dispatch()
}
