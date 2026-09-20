//! Outcome-driven relation re-evaluation (D36/D37/C10).
//!
//! Pure patch construction plus the trusted writer envelope. The operation key
//! is the patch id, so re-sending the same outcome is a no-op and a second
//! outcome from the same prior version loses the CAS race. No new operation
//! table is created.

use super::validation_v2::load_existing_versioned;
use crate::memory::personal_state::store;
use crate::persistence::sqlite::SqliteWriter;
use rusqlite::Connection;
use saaa_personal_state_core::world::identity_v2;
use saaa_personal_state_core::world::model::Stance;
use saaa_personal_state_core::world::model_v2::{Epistemic, RelationPayloadV2, WorldPayloadV2};
use saaa_personal_state_core::world::outcome_v2::{
    compare_outcome, Outcome, OutcomeVerdict, Prediction,
};
use saaa_personal_state_core::world::versioned::{payload_ref_v2, WorldView};
use saaa_personal_state_core::*;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone)]
pub struct PreparedOutcomePatch {
    pub operation_key: String,
    pub patch: StatePatch,
    pub payloads: BTreeMap<String, serde_json::Value>,
    pub fingerprint: String,
    pub new_assertion_id: String,
}

fn operation_key(
    project_scope: &str,
    prior_assertion_id: &str,
    outcome_source: &SourceKey,
) -> String {
    let payload = serde_json::json!([
        project_scope,
        prior_assertion_id,
        outcome_source.id,
        outcome_source.version
    ]);
    format!(
        "wmo2:{}",
        sha256(&serde_json::to_vec(&payload).expect("json"))
    )
}

fn sha256(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    format!("{:x}", Sha256::digest(bytes))
}

/// Build the disputed successor of an Active relation from an observed
/// counterexample. Returns `InvalidPayload` if the inputs are not comparable or
/// do not actually contradict the prediction.
pub fn prepare_outcome_patch(
    c: &Connection,
    ledger: &Ledger,
    project_scope: &str,
    prior_assertion_id: &str,
    prediction: &Prediction,
    outcome: &Outcome,
    now: i64,
) -> Result<PreparedOutcomePatch, String> {
    let prior = ledger
        .assertions
        .get(prior_assertion_id)
        .ok_or("world-invalid-reference")?;
    if prior.kind != Kind::WorldRelation || ledger.status(prior_assertion_id, now) != Status::Active
    {
        return Err("world-invalid-reference".into());
    }
    let existing = load_existing_versioned(c, ledger)?;
    let Some(prior_payload) = existing.get(&prior.payload_ref) else {
        return Err("world-projection-corrupt".into());
    };
    let WorldView::Relation(prior_view) = prior_payload.view(&prior.semantic_key) else {
        return Err("world-invalid-payload".into());
    };
    let prior_relation = prior_view.payload;
    let comparison = compare_outcome(
        &prior_relation,
        prior.valid_from,
        prior.valid_until,
        prior_relation.confidence.as_ref(),
        prediction,
        outcome,
    )
    .map_err(|e| e.code().to_string())?;
    if comparison.verdict != OutcomeVerdict::Counterexample || !comparison.disputed {
        return Err("world-invalid-payload".into());
    }
    let new_id = crate::new_id("assertion");
    let mut next: RelationPayloadV2 = prior_relation.clone();
    next.epistemic = Epistemic::Disputed;
    next.confidence = comparison.confidence.clone();
    next.outcome_update = Some(saaa_personal_state_core::world::model_v2::OutcomeUpdate {
        prior_assertion_id: prior_assertion_id.to_string(),
        outcome_source: outcome.source.clone(),
        prediction_source: prediction.source.clone(),
        comparison_id: outcome
            .comparison_id
            .clone()
            .ok_or("world-invalid-payload")?,
        expected: prediction.direction,
        actual: outcome.direction,
        predicted_at_ms: prediction.at_ms,
        observed_at_ms: outcome.at_ms,
    });
    // The new relation must declare every source that produced the assessment.
    for source in [&outcome.source, &prediction.source] {
        if !next.assessment_refs.contains(source) {
            next.assessment_refs.push(source.clone());
        }
    }
    for source in [&outcome.source, &prediction.source] {
        if !next
            .evidence_stances
            .iter()
            .any(|stance| &stance.source == source)
        {
            next.evidence_stances
                .push(saaa_personal_state_core::world::model::EvidenceStance {
                    source: source.clone(),
                    stance: Stance::Challenges,
                });
        }
    }
    let canonical = WorldPayloadV2::Relation(next.clone())
        .canonical_bytes()
        .map_err(|_| "world-invalid-payload")?;
    let payload_ref = payload_ref_v2(&canonical);
    let semantic_key = identity_v2::relation_key_from_payload(
        project_scope,
        &next,
        prior.valid_from,
        prior.valid_until,
    );

    let mut evidence = prior.evidence.clone();
    evidence.insert(outcome.source.clone());
    evidence.insert(prediction.source.clone());
    let mut inputs = prior.input_dependencies.clone();
    inputs.insert(outcome.source.clone());
    inputs.insert(prediction.source.clone());

    let assertion = Assertion {
        id: new_id.clone(),
        kind: Kind::WorldRelation,
        semantic_key,
        payload_ref: payload_ref.clone(),
        access: prior.access.clone(),
        evidence,
        // The superseded prior version is never a generation dependency; the
        // endpoints and inherited inputs are.
        depends_on: prior.depends_on.clone(),
        input_dependencies: inputs.clone(),
        provenance: prior.provenance.clone(),
        observed_at: now,
        effective_at: now,
        recorded_at: now,
        valid_from: prior.valid_from,
        valid_until: prior.valid_until,
    };
    let key = operation_key(project_scope, prior_assertion_id, &outcome.source);
    let mut transitions = Vec::new();
    let mut sequence = ledger.transitions.last().map_or(1, |t| t.sequence + 1);
    for action in [
        Action::Assert,
        Action::Supersede { by: new_id.clone() },
        Action::Activate,
        Action::Dispute,
    ] {
        let assertion_id = if matches!(action, Action::Supersede { .. }) {
            prior_assertion_id.to_string()
        } else {
            new_id.clone()
        };
        transitions.push(Transition {
            id: crate::new_id("transition"),
            sequence,
            assertion_id,
            action,
            reason_code: "counterevidence-v1".into(),
            evidence: assertion.evidence.clone(),
            input_dependencies: inputs.clone(),
            recorded_at: now,
        });
        sequence += 1;
    }
    let patch = StatePatch {
        id: key.clone(),
        base_revision: ledger.revision,
        input_epoch: ledger.input_epoch,
        policy_revision: ledger.policy_revision,
        fence: String::new(),
        assertions: vec![assertion],
        transitions,
        coverage: Vec::new(),
    };
    let fingerprint = patch_fingerprint(&patch).map_err(|e| e.to_string())?;
    let payload_value: serde_json::Value =
        serde_json::from_slice(&canonical).map_err(|_| "world-invalid-payload")?;
    let payloads = BTreeMap::from([(payload_ref, payload_value)]);
    Ok(PreparedOutcomePatch {
        operation_key: key,
        patch,
        payloads,
        fingerprint,
        new_assertion_id: new_id,
    })
}

/// Commit a prepared outcome patch through the real single Writer. The fence is
/// supplied by the trusted caller; the operation key is the patch id.
pub fn commit_prepared_outcome(
    writer: &SqliteWriter,
    prepared: &PreparedOutcomePatch,
    fence: &str,
) -> Result<bool, String> {
    let mut patch = prepared.patch.clone();
    patch.fence = fence.to_string();
    let mut payloads = prepared.payloads.clone();
    // The new relation is the only assertion; the payload map must contain its ref.
    let new_assertion = patch
        .assertions
        .first()
        .expect("prepared patch has one assertion");
    if !payloads.contains_key(&new_assertion.payload_ref) {
        return Err("world-invalid-payload".into());
    }
    let inputs: BTreeSet<SourceKey> = patch
        .assertions
        .iter()
        .flat_map(|a| a.input_dependencies.iter().cloned())
        .collect();
    let payload_bytes = payloads
        .iter()
        .map(|(k, v)| {
            Ok((
                k.clone(),
                serde_json::to_vec(v)
                    .map_err(|_| "world-invalid-payload".to_string())?
                    .len(),
            ))
        })
        .collect::<Result<BTreeMap<_, _>, String>>()?;
    let payload_id = new_assertion.payload_ref.clone();
    let canonical = payloads
        .remove(&payload_id)
        .ok_or("world-invalid-payload")?;
    let mut stored = BTreeMap::new();
    stored.insert(payload_id, canonical);
    writer
        .write(|c| {
            let ledger = store::load(c)?;
            let context = CommitContext {
                access: AccessRequest {
                    principal: &ledger.principal,
                    scope: "primary",
                    task_request: patch.assertions[0].access.task_request.as_deref(),
                    purpose: Purpose::StateExtract,
                    max_classification: Classification::Confidential,
                    policy_revision: ledger.policy_revision,
                    authorized: true,
                },
                enabled: true,
                now: patch.assertions[0].recorded_at,
                live_fence: fence,
                issued_patch_id: &prepared.operation_key,
                issued_assertion_ids: BTreeSet::from([prepared.new_assertion_id.clone()]),
                issued_payload_bytes: payload_bytes,
                evidence_allowlist: inputs.clone(),
                input_dependencies: inputs,
            };
            store::commit(c, &patch, &context, &stored)
        })
        .map_err(|e| e.to_string())
}
