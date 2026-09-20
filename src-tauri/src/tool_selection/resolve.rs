//! Stable tool-reference resolution for correction feedback. Kept separate from the correction
//! transaction so a name that appears in two external sources can never be guessed with `LIMIT 1`.

use rusqlite::Connection;

use super::contracts::RequestContext;
use super::repository;

/// Resolves an extraction's tool reference to a stable catalog id without ever guessing between
/// same-named tools from different sources. Resolution order:
///
/// 1. an exact catalog id that the principal is authorized for;
/// 2. an explicit `sourceId/toolName` qualifier that is unique and authorized;
/// 3. a name that is unique in the referenced decision's candidates; otherwise
/// 4. a name that is unique across the principal's authorized tools.
///
/// Any remaining ambiguity returns `None` (the caller records an ambiguous correction).
pub fn resolve_tool_id(
    connection: &Connection,
    value: &str,
    context: &RequestContext,
    decision_id: Option<&str>,
) -> Option<String> {
    let authorized = |tool_id: &str| {
        repository::grant_exists(
            connection,
            &context.principal_id,
            tool_id,
            context.project_id.as_deref(),
        )
        .unwrap_or(false)
    };
    if let Ok(Some(tool)) = repository::tool_by_id(connection, value) {
        if authorized(&tool.id) {
            return Some(tool.id);
        }
    }
    if let Some((source_id, tool_name)) = value.split_once('/') {
        let candidates: Vec<String> = repository::tool_ids_by_name(connection, tool_name)
            .unwrap_or_default()
            .into_iter()
            .filter(|tool_id| {
                repository::tool_by_id(connection, tool_id)
                    .ok()
                    .flatten()
                    .map(|tool| tool.source_id == source_id)
                    .unwrap_or(false)
            })
            .filter(|tool_id| authorized(tool_id))
            .collect();
        return unique(candidates);
    }
    let name_matches: Vec<String> = repository::tool_ids_by_name(connection, value)
        .unwrap_or_default()
        .into_iter()
        .filter(|tool_id| authorized(tool_id))
        .collect();
    if let Some(decision_id) = decision_id {
        let candidates: std::collections::HashSet<String> =
            repository::decision_by_id(connection, decision_id)
                .ok()
                .flatten()
                .map(|decision| {
                    decision
                        .candidates
                        .into_iter()
                        .map(|candidate| candidate.tool_id)
                        .collect()
                })
                .unwrap_or_default();
        let narrowed: Vec<String> = name_matches
            .iter()
            .filter(|tool_id| candidates.contains(tool_id.as_str()))
            .cloned()
            .collect();
        if !narrowed.is_empty() {
            return unique(narrowed);
        }
    }
    unique(name_matches)
}

fn unique(values: Vec<String>) -> Option<String> {
    if values.len() == 1 {
        values.into_iter().next()
    } else {
        None
    }
}
