use super::*;
pub(super) fn select(
    ledger: &saaa_personal_state_core::Ledger,
    source: &SourceRef,
    input: &Value,
    world: bool,
) -> Result<Vec<SourceRef>, String> {
    let mut sources = context::initial(source, input, world)?;
    for assertion in ledger.assertions.values() {
        if world {
            if assertion.access.task_request.as_deref() != input["request_scope"].as_str()
                || !(assertion.kind.is_world()
                    || assertion.kind == saaa_personal_state_core::Kind::Objective)
            {
                continue;
            }
        } else if assertion.kind.is_world() {
            continue;
        }
        for key in &assertion.input_dependencies {
            if let Some(s) = ledger.sources.get(key) {
                if !sources.contains(s) {
                    sources.push(s.clone());
                }
            }
        }
    }
    Ok(sources)
}

#[path = "product_extract_sources/context.rs"]
mod context;
