//! Host-owned comparison of quoted user outcomes. No inferred numbers or promotions.
use super::{outcome_v2, validation_v2::load_existing_versioned};
use crate::memory::personal_state::store;
use rusqlite::{Connection, OptionalExtension};
use saaa_personal_state_core::{
    world::{
        extraction::Extraction,
        model_v2::{EffectDirection, RelationTypeV2},
        outcome_v2::{compare_outcome, Outcome, OutcomeVerdict, Prediction},
        versioned::WorldView,
    },
    SourceRef, Status,
};

pub(super) fn commit(
    c: &Connection,
    extracted: &Extraction,
    source: &SourceRef,
    scope: &str,
    fence: &str,
    now: i64,
) -> Result<(), String> {
    if !extracted.outcomes.is_empty()
        && (!source.finalized || source.role != saaa_personal_state_core::SourceRole::User)
    {
        return Err("world-extraction-evidence".into());
    }
    for observation in &extracted.outcomes {
        let ledger = store::load(c)?;
        let prior = ledger
            .assertions
            .get(&observation.prior_assertion_id)
            .ok_or("world-invalid-reference")?;
        if prior.access.task_request.as_deref() != Some(scope) {
            return Err("world-scope-denied".into());
        }
        if !saaa_personal_state_core::world::validation_v2::is_knowledge_scope(
            scope,
            &ledger.principal,
        ) {
            return Err("world-scope-denied".into());
        }
        for key in prior
            .input_dependencies
            .iter()
            .chain(std::iter::once(&source.key))
        {
            let mapped: bool = c.query_row("SELECT EXISTS(SELECT 1 FROM personal_source_scope_refs r JOIN context_scopes s ON s.scope_key=r.scope_key WHERE r.source_id=?1 AND r.version=?2 AND r.scope_key=?3 AND s.state='active')",rusqlite::params![key.id,key.version,scope],|r|r.get(0)).map_err(crate::database_error)?;
            if !mapped || !ledger.source_valid(key, now) {
                return Err("world-scope-denied".into());
            }
        }
        let id = outcome_v2::operation_key(scope, &prior.id, &source.key);
        let mut metadata = serde_json::json!({"metric":observation.metric_entity_id,"comparison":observation.comparison_id,"conditions":observation.conditions,"direction":observation.direction,"quoteStart":observation.quote_start,"quoteEnd":observation.quote_end});
        let prior_record: Option<String> = c
            .query_row(
                "SELECT value_json FROM personal_world_observations WHERE id=?1",
                [&id],
                |r| r.get(0),
            )
            .optional()
            .map_err(crate::database_error)?;
        if let Some(raw) = prior_record {
            let mut saved: serde_json::Value =
                serde_json::from_str(&raw).map_err(|_| "world-projection-corrupt")?;
            saved
                .as_object_mut()
                .ok_or("world-projection-corrupt")?
                .remove("verdict");
            if saved != metadata {
                return Err("world-outcome-conflict".into());
            }
            continue;
        }
        if ledger.status(&prior.id, now) != Status::Active {
            return Err("world-invalid-reference".into());
        }
        let existing = load_existing_versioned(c, &ledger)?;
        let payload = existing
            .get(&prior.payload_ref)
            .ok_or("world-projection-corrupt")?;
        let WorldView::Relation(relation) = payload.view(&prior.semantic_key) else {
            return Err("world-invalid-payload".into());
        };
        let direction = match relation.payload.relation_type {
            RelationTypeV2::Increases => EffectDirection::Increase,
            RelationTypeV2::Decreases => EffectDirection::Decrease,
            _ => return Err("world-invalid-payload".into()),
        };
        let prediction_source = prior
            .evidence
            .iter()
            .next()
            .ok_or("world-invalid-reference")?
            .clone();
        let mut conditions = relation
            .payload
            .conditions
            .iter()
            .map(|v| (v.key.clone(), v.value.clone()))
            .collect::<Vec<_>>();
        conditions.sort();
        let prediction = Prediction {
            metric_id: relation.payload.to_entity_id.clone(),
            comparison_id: relation.payload.comparison_id.clone(),
            conditions,
            direction,
            at_ms: prior.observed_at,
            source: prediction_source,
        };
        let mut conditions = observation
            .conditions
            .iter()
            .map(|v| (v.key.clone(), v.value.clone()))
            .collect::<Vec<_>>();
        conditions.sort();
        let outcome = Outcome {
            metric_id: observation.metric_entity_id.clone(),
            comparison_id: Some(observation.comparison_id.clone()),
            conditions,
            direction: observation.direction,
            at_ms: source.recorded_at,
            source: source.key.clone(),
        };
        let comparison = compare_outcome(
            &relation.payload,
            prior.valid_from,
            prior.valid_until,
            relation.payload.confidence.as_ref(),
            &prediction,
            &outcome,
        )
        .map_err(|e| e.code().to_string())?;
        metadata["verdict"] = serde_json::json!(format!("{:?}", comparison.verdict));
        let raw = metadata.to_string();
        if raw.len() > 2000 {
            return Err("world-limit".into());
        }
        c.execute(
            "INSERT OR IGNORE INTO personal_world_observations VALUES(?1,?2,?3,?4,?5,?6,?7)",
            rusqlite::params![
                id,
                scope,
                prior.id,
                source.key.id,
                source.key.version,
                raw,
                source.recorded_at
            ],
        )
        .map_err(crate::database_error)?;
        let mut dependencies = prior.input_dependencies.clone();
        dependencies.insert(source.key.clone());
        for dependency in dependencies {
            c.execute(
                "INSERT OR IGNORE INTO personal_world_observation_sources VALUES(?1,?2,?3)",
                rusqlite::params![id, dependency.id, dependency.version],
            )
            .map_err(crate::database_error)?;
        }
        if comparison.verdict != OutcomeVerdict::Counterexample {
            continue;
        }
        let prepared = outcome_v2::prepare_outcome_patch(
            c,
            &ledger,
            scope,
            &prior.id,
            &prediction,
            &outcome,
            now,
        )?;
        outcome_v2::commit_prepared_connection(c, &prepared, fence)?;
    }
    Ok(())
}
