//! Primary, contextual and pending source registration selection.
use super::*;
pub(super) fn initial(
    source: &SourceRef,
    input: &Value,
    world: bool,
) -> Result<Vec<SourceRef>, String> {
    let mut sources = vec![source.clone()];
    if world {
        for entry in input["context_sources"].as_array().into_iter().flatten() {
            let reference: SourceRef = serde_json::from_value(entry["ref"].clone())
                .map_err(|_| "world-extraction-evidence")?;
            if !sources.contains(&reference) {
                sources.push(reference);
            }
        }
    }
    if let Some(pending) = input["current"]["pending"].as_array() {
        for entry in pending {
            let s: SourceRef = serde_json::from_value(entry["source"].clone())
                .map_err(|_| "personal-base-source")?;
            if !sources.contains(&s) {
                sources.push(s);
            }
        }
    }
    Ok(sources)
}
