use super::*;

/// Installs an explicit developer fixture only in `mode: "mock"`.  This path is opt-in through
/// the process configuration; it never runs for direct or real-model discovery and its backend
/// returns deterministic fixture data rather than contacting a capability or remote service.
pub(super) fn seed_mock_catalog_and_policy(
    service: &ToolSelectionService,
    writer: &std::sync::Arc<crate::persistence::SqliteWriter>,
) {
    use catalog::{CatalogEntry, UsagePage};
    use rusqlite::OptionalExtension;
    use serde_json::json;

    let Ok(principal) = service::ensure_principal(writer) else {
        return;
    };
    let entry = |tool_id: &str, title: &str, purpose: &str| CatalogEntry {
        tool_id: tool_id.to_string(),
        backend_key: tool_id.to_string(),
        title: title.to_string(),
        purpose: purpose.to_string(),
        operations: vec!["search".to_string(), "read".to_string()],
        objects: vec!["development_fixture".to_string()],
        suitable: vec!["adaptive Tool-selection development tests".to_string()],
        unsuitable: vec!["production data or external side effects".to_string()],
        required_inputs: vec!["q".to_string()],
        input_schema: json!({
            "type": "object",
            "properties": { "q": { "type": "string" } },
            "required": ["q"],
            "additionalProperties": false
        }),
        output_schema: None,
        effect: "read",
        usage_pages: vec![UsagePage {
            section: "usage",
            page: 0,
            text:
                "Development-only deterministic fixture. It does not access user or external data."
                    .to_string(),
        }],
        backend_binding: json!({"kind":"fixture", "fixture": true}),
    };
    let entries = [
        entry(
            "adaptive-fixture-web",
            "Development fixture: web",
            "Deterministic stand-in for the rule-ranked web candidate.",
        ),
        entry(
            "adaptive-fixture-minutes",
            "Development fixture: minutes",
            "Deterministic stand-in for the learned preferred candidate.",
        ),
        entry(
            "adaptive-fixture-archive",
            "Development fixture: archive",
            "Deterministic stand-in for an alternate eligible candidate.",
        ),
    ];
    if service
        .ingest_catalog(&principal, "adaptive-development-fixture", &entries)
        .is_err()
    {
        return;
    }
    let _ = writer.write(|connection| {
        let candidates = vec![
            "adaptive-fixture-web-rev1".to_string(),
            "adaptive-fixture-minutes-rev1".to_string(),
            "adaptive-fixture-archive-rev1".to_string(),
        ];
        let existing_active: Option<String> = connection
            .query_row(
                "SELECT id FROM ai_artifacts WHERE domain='tool' AND scope_key='global' AND state='active' AND candidate_fingerprint=?1 LIMIT 1",
                [crate::adaptive_improvement::fingerprint_for(&candidates)],
                |row| row.get(0),
            )
            .optional()
            .map_err(|error| error.to_string())?;
        let artifact = if let Some(artifact) = existing_active {
            artifact
        } else {
            for (id, selected, successful) in [
                ("adaptive-fixture-outcome-web", "adaptive-fixture-web-rev1", false),
                (
                    "adaptive-fixture-outcome-minutes",
                    "adaptive-fixture-minutes-rev1",
                    true,
                ),
                (
                    "adaptive-fixture-outcome-archive",
                    "adaptive-fixture-archive-rev1",
                    false,
                ),
            ] {
                let exists: bool = connection
                    .query_row("SELECT EXISTS(SELECT 1 FROM ai_decisions WHERE id=?1)", [id], |row| row.get(0))
                    .map_err(|error| error.to_string())?;
                if !exists {
                    crate::adaptive_improvement::record_decision(
                        connection,
                        &crate::adaptive_improvement::DecisionObservation {
                            id: id.to_string(),
                            domain: crate::adaptive_improvement::Domain::Tool,
                            scope_key: "global".to_string(),
                            event_seq: 0,
                            policy_revision: 0,
                            candidate_fingerprint: crate::adaptive_improvement::fingerprint_for(&candidates),
                            eligible_candidates: candidates.clone(),
                            selected: selected.to_string(),
                            selection_mode: "rules".to_string(),
                            source_refs_json: json!({"fixture": "adaptive-development"}).to_string(),
                        },
                        0,
                    )?;
                    crate::adaptive_improvement::record_outcome(
                        connection,
                        id,
                        Some(successful),
                        None,
                        None,
                        None,
                        None,
                        None,
                        None,
                        "adaptive-development-fixture",
                        1,
                        0,
                    )?;
                }
            }
            let dataset = crate::adaptive_improvement::materialize_dirty(connection, 100, 0)?
                .ok_or_else(|| "mock fixture did not materialize its dataset".to_string())?;
            crate::adaptive_improvement::train_candidate_artifacts(connection, &dataset, 0)?
                .into_iter()
                .next()
                .ok_or_else(|| "mock fixture did not train a Tool artifact".to_string())?
        };
        let state: String = connection
            .query_row(
                "SELECT state FROM ai_artifacts WHERE id=?1",
                [&artifact],
                |row| row.get(0),
            )
            .map_err(|error| error.to_string())?;
        if state == "candidate" {
            crate::adaptive_improvement::apply_evaluation_gate(
                connection,
                &artifact,
                crate::adaptive_improvement::EvaluationGate {
                    examples: 200,
                    recipe_examples: 30,
                    independent_groups: 20,
                    protocol_errors: 0,
                    invalid_sources: 0,
                    scope_leaks: 0,
                    unknown_candidates: 0,
                    success_ci_lower: 0.01,
                    resource_improvement_ci_lower: Some(0.05),
                    other_resource_regression_upper: 0.0,
                },
            )?;
        }
        let state: String = connection
            .query_row(
                "SELECT state FROM ai_artifacts WHERE id=?1",
                [&artifact],
                |row| row.get(0),
            )
            .map_err(|error| error.to_string())?;
        if state == "shadow" {
            crate::adaptive_improvement::approve_shadow(connection, &artifact)?;
        }
        let state: String = connection
            .query_row(
                "SELECT state FROM ai_artifacts WHERE id=?1",
                [&artifact],
                |row| row.get(0),
            )
            .map_err(|error| error.to_string())?;
        if state == "eligible" {
            crate::adaptive_improvement::activate(connection, &artifact, 1, 0)?;
        }
        let mut settings = crate::persistence::load_role_routing_settings(connection)?;
        settings.adaptive_improvement.enabled = true;
        settings.adaptive_improvement.tool = true;
        connection
            .execute(
                "UPDATE settings_documents SET value_json=?1 WHERE namespace='routing.roles' AND key='default'",
                [serde_json::to_string(&settings).map_err(|error| error.to_string())?],
            )
            .map_err(|error| error.to_string())?;
        Ok(())
    });
}
