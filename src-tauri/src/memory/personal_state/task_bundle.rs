//! Optional state returned by the reasoning generation, committed with its answer.
use super::{generation, store, worker::Candidate};
use rusqlite::Connection;
use saaa_personal_state_core::*;
use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SupportedCandidate {
    pub candidate: Candidate,
    pub evidence: BTreeSet<SourceKey>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Answer {
    pub answer: String,
    pub state: Vec<SupportedCandidate>,
}

/// Caller holds the same transaction used for saving the answer and completing run.
pub fn adopt(
    c: &Connection,
    m: &generation::Manifest,
    candidates: Vec<SupportedCandidate>,
    provenance: Provenance,
) -> Result<(), String> {
    generation::allow(c, &m.generation_id)?;
    if candidates.is_empty() {
        return Ok(());
    }
    if candidates.len() > 10 {
        return Err("personal-task-patch-budget".into());
    }
    let source = m.sources.first().ok_or("personal-task-source")?;
    let request = source.key.id.as_str();
    let mut ledger = store::load(c)?;
    if (ledger.revision, ledger.input_epoch, ledger.policy_revision)
        != (m.projection_revision, m.input_epoch, m.policy_revision)
    {
        return Err("personal-task-patch-stale".into());
    }
    for source in &m.sources {
        store::remember_source(c, source)?;
    }
    ledger = store::load(c)?;
    let inputs = m
        .sources
        .iter()
        .map(|s| s.key.clone())
        .collect::<BTreeSet<_>>();
    let id = crate::new_id("patch");
    let now = super::now();
    let mut patch = StatePatch {
        id: id.clone(),
        base_revision: ledger.revision,
        input_epoch: m.input_epoch,
        policy_revision: m.policy_revision,
        fence: m.attempt_id.clone(),
        assertions: Vec::new(),
        transitions: Vec::new(),
        coverage: Vec::new(),
    };
    let mut payloads = BTreeMap::new();
    let mut sequence = ledger.transitions.last().map_or(1, |t| t.sequence + 1);
    for supported in candidates {
        let candidate = supported.candidate;
        if supported.evidence.is_empty()
            || !supported.evidence.is_subset(&inputs)
            || !matches!(candidate.status, Status::Active | Status::Candidate)
            || candidate
                .task_request
                .as_deref()
                .is_some_and(|id| id != request)
        {
            return Err("personal-task-patch-scope".into());
        }
        let a_id = crate::new_id("assertion");
        let payload = crate::new_id("payload");
        let mut access = source.access.clone();
        access.task_request = candidate.task_request;
        let assertion = Assertion {
            id: a_id.clone(),
            kind: candidate.kind,
            semantic_key: candidate.semantic_key,
            payload_ref: payload.clone(),
            access,
            evidence: supported.evidence.clone(),
            depends_on: BTreeSet::new(),
            input_dependencies: inputs.clone(),
            provenance: provenance.clone(),
            observed_at: source.recorded_at,
            effective_at: now,
            recorded_at: now,
            valid_from: now,
            valid_until: None,
        };
        let mut actions = vec![(a_id.clone(), Action::Assert)];
        if let Some(old) = candidate.replaces {
            actions.push((old, Action::Supersede { by: a_id.clone() }));
        }
        if candidate.status == Status::Active {
            actions.push((a_id, Action::Activate));
        }
        for (target, action) in actions {
            patch.transitions.push(Transition {
                id: crate::new_id("transition"),
                sequence,
                assertion_id: target,
                action,
                reason_code: "task-supported".into(),
                evidence: supported.evidence.clone(),
                input_dependencies: inputs.clone(),
                recorded_at: now,
            });
            sequence += 1;
        }
        payloads.insert(payload, candidate.value);
        patch.assertions.push(assertion);
    }
    // Task-proposed items are not a claim that all input was extracted. Coverage
    // stays pending until the dedicated worker has scanned the full source.
    let context = CommitContext {
        access: AccessRequest {
            principal: &ledger.principal,
            scope: "primary",
            task_request: Some(request),
            purpose: Purpose::StateExtract,
            max_classification: Classification::Confidential,
            policy_revision: m.policy_revision,
            authorized: true,
        },
        enabled: true,
        now,
        live_fence: &m.attempt_id,
        issued_patch_id: &id,
        issued_assertion_ids: patch.assertions.iter().map(|a| a.id.clone()).collect(),
        issued_payload_bytes: payloads
            .iter()
            .map(|(id, value)| Ok((id.clone(), super::encode(value)?.len())))
            .collect::<Result<_, String>>()?,
        evidence_allowlist: inputs.clone(),
        input_dependencies: inputs,
    };
    store::commit(c, &patch, &context, &payloads)?;
    Ok(())
}
