//! Validated query observations (D22/C4/C7).
//!
//! A request may carry condition and availability observations, but they are
//! only trusted after the adapter checks the Source exists, is finalized and
//! available, is still mapped to the project and permits this request. Anything
//! that fails is dropped with an `observation_unavailable` notice; it is never
//! treated as "validated". At most 30 observations are accepted.

use super::super::sources;
use rusqlite::Connection;
use saaa_personal_state_core::world::conditions_v2::{
    AvailabilityObservation, AvailabilityValue, ConditionObservation,
};
use saaa_personal_state_core::*;

pub const OBSERVATION_UNAVAILABLE: &str = "observation_unavailable";
pub const MAX_OBSERVATIONS: usize = 30;

#[derive(Debug, Clone)]
pub struct ConditionObservationInput {
    pub relation_assertion_id: String,
    pub key: String,
    pub value: String,
    pub source: SourceKey,
}

#[derive(Debug, Clone)]
pub struct AvailabilityObservationInput {
    pub entity_id: String,
    pub value: AvailabilityValue,
    pub source: SourceKey,
}

#[derive(Debug, Default)]
pub struct ValidatedObservations {
    pub conditions: Vec<ConditionObservation>,
    pub availability: Vec<AvailabilityObservation>,
    pub notices: Vec<String>,
}

fn source_usable(
    c: &Connection,
    ledger: &Ledger,
    access: &AccessRequest<'_>,
    project_scope: &str,
    key: &SourceKey,
    now: i64,
) -> bool {
    let Some(source) = ledger.sources.get(key) else {
        return false;
    };
    if !source.finalized || !source.access.permits(access) || !ledger.source_valid(key, now) {
        return false;
    }
    if sources::revalidate(c, source).is_err() {
        return false;
    }
    let reference = SourceKey {
        id: key.id.clone(),
        version: key.version,
        start: key.start,
        end: key.end,
    };
    let mapped: Result<bool, _> = c.query_row(
        "SELECT EXISTS(SELECT 1 FROM personal_source_scope_refs
           WHERE source_id=?1 AND version=?2 AND scope_key=?3)",
        rusqlite::params![reference.id, reference.version, project_scope],
        |r| r.get(0),
    );
    matches!(mapped, Ok(true))
}

/// Validate at most 30 observations. Expired, unmapped, unauthorized or
/// revalidated-away observations are dropped with one notice.
pub fn validate_observations(
    c: &Connection,
    ledger: &Ledger,
    access: &AccessRequest<'_>,
    project_scope: &str,
    now: i64,
    conditions: &[ConditionObservationInput],
    availability: &[AvailabilityObservationInput],
) -> Result<ValidatedObservations, String> {
    if conditions.len() + availability.len() > MAX_OBSERVATIONS {
        return Err("world-limit".into());
    }
    let mut out = ValidatedObservations::default();
    for item in conditions {
        // The observation must name a World relation this request may see; an
        // arbitrary id is never treated as validated.
        let relation_ok = ledger
            .assertions
            .get(&item.relation_assertion_id)
            .is_some_and(|assertion| {
                assertion.kind == saaa_personal_state_core::Kind::WorldRelation
                    && assertion.access.permits(access)
            });
        if !relation_ok || !source_usable(c, ledger, access, project_scope, &item.source, now) {
            out.notices.push(OBSERVATION_UNAVAILABLE.into());
            continue;
        }
        out.conditions.push(ConditionObservation {
            relation_assertion_id: item.relation_assertion_id.clone(),
            key: item.key.clone(),
            value: item.value.clone(),
            evidence: item.source.clone(),
            valid_from_ms: 0,
            valid_until_ms: None,
        });
    }
    for item in availability {
        if !source_usable(c, ledger, access, project_scope, &item.source, now) {
            out.notices.push(OBSERVATION_UNAVAILABLE.into());
            continue;
        }
        out.availability.push(AvailabilityObservation {
            entity_id: item.entity_id.clone(),
            value: item.value,
            evidence: item.source.clone(),
            valid_from_ms: 0,
            valid_until_ms: None,
        });
    }
    out.notices.sort();
    out.notices.dedup();
    Ok(out)
}
