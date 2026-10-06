//! Typed patch construction. No model IO or commits occur here.
use super::*;
pub(super) struct Input<'a> {
    pub c: &'a rusqlite::Connection,
    pub chunk: &'a sources::Chunk,
    pub ledger: &'a Ledger,
    pub job: &'a jobs::Job,
    pub extractor: &'a dyn Extractor,
    pub current: &'a [Value],
    pub dependencies: BTreeSet<SourceKey>,
    pub extraction: Extraction,
    pub request_scope: Option<String>,
    pub patch_id: String,
    pub fence: String,
    pub now: i64,
    pub consolidating: bool,
    pub exposed: &'a BTreeSet<String>,
}
pub(super) struct Built {
    pub patch: StatePatch,
    pub payloads: BTreeMap<String, Value>,
    pub payload_bytes: BTreeMap<String, usize>,
    pub dependencies: BTreeSet<SourceKey>,
    pub request_scope: Option<String>,
}
pub(super) fn build(input: Input<'_>) -> Result<Built, String> {
    let Input {
        c,
        chunk,
        ledger,
        job,
        extractor,
        current,
        dependencies,
        extraction,
        request_scope,
        patch_id,
        fence,
        now,
        consolidating,
        exposed,
    } = input;
    let mut patch = StatePatch {
        id: patch_id.clone(),
        base_revision: ledger.revision,
        input_epoch: job.epoch,
        policy_revision: ledger.policy_revision,
        fence: fence.clone(),
        assertions: Vec::new(),
        transitions: Vec::new(),
        coverage: Vec::new(),
    };
    let mut payloads = BTreeMap::new();
    let mut next_sequence = ledger.transitions.last().map_or(1, |t| t.sequence + 1);
    for mut candidate in extraction.candidates {
        if consolidating && !candidate.semantic_key.starts_with("consolidated:") {
            candidate.semantic_key = format!("consolidated:{}", candidate.semantic_key);
        }
        crate::memory::personal_state::admission::gate(&mut candidate, &chunk.source, &chunk.text)?;
        if let Some(old_id) = &candidate.replaces {
            let old = ledger
                .assertions
                .get(old_id)
                .ok_or("personal-replacement-unknown")?;
            if !exposed.contains(old_id)
                || old.kind != candidate.kind
                || old.semantic_key != candidate.semantic_key
                || old.access.task_request.as_deref() != request_scope.as_deref()
            {
                return Err("personal-replacement-mismatch".into());
            }
        }
        if !chunk.source.finalized {
            candidate.status = Status::Candidate;
            candidate.replaces = None;
        }
        if !matches!(candidate.status, Status::Active | Status::Candidate)
            || candidate.task_request.as_deref() != request_scope.as_deref()
        {
            return Err("personal-extraction-scope".into());
        }
        // New/backlog fairness must never allow an older observation to replace
        // a more recent current assertion about the same topic.
        if ledger.assertions.values().any(|a| {
            a.kind == candidate.kind
                && a.semantic_key == candidate.semantic_key
                && a.access.task_request.as_deref() == request_scope.as_deref()
                && (a.effective_at
                    > candidate
                        .support
                        .effective_at
                        .unwrap_or(chunk.source.recorded_at)
                    || a.observed_at > chunk.source.recorded_at)
                && ledger.status(&a.id, now) == Status::Active
        }) {
            candidate.status = Status::Candidate;
            candidate.replaces = None;
        }
        let mut ambiguous = Vec::new();
        if candidate.kind.is_profile()
            && candidate.status == Status::Active
            && candidate.replaces.is_none()
        {
            for old in ledger.assertions.values().filter(|a| {
                a.kind == candidate.kind
                    && a.semantic_key == candidate.semantic_key
                    && a.access.task_request.as_deref() == request_scope.as_deref()
                    && ledger.status(&a.id, now) == Status::Active
            }) {
                let raw: String = c
                    .query_row(
                        "SELECT value_json FROM personal_payloads WHERE id=?1",
                        [&old.payload_ref],
                        |r| r.get(0),
                    )
                    .map_err(database_error)?;
                let value: Value = crate::memory::personal_state::decode(raw)?;
                let same = if value["policy"] == "personal-gate-v1" {
                    value["value"] == candidate.value
                } else {
                    value == candidate.value
                };
                if !same {
                    ambiguous.push(old.id.clone());
                }
            }
            if !ambiguous.is_empty() {
                candidate.status = Status::Candidate;
            }
        }
        let id = crate::new_id("assertion");
        let payload = crate::new_id("payload");
        let mut access = chunk.source.access.clone();
        access.task_request = candidate.task_request;
        let mut effective_at = candidate
            .support
            .effective_at
            .unwrap_or(chunk.source.recorded_at);
        if candidate.support.effective_at.is_none() && candidate.support.valid_until.is_none() {
            if let Some(old) = ledger.assertions.values().find(|a| {
                a.kind == candidate.kind
                    && a.semantic_key == candidate.semantic_key
                    && a.access.task_request == access.task_request
                    && ledger.status(&a.id, now) == Status::Active
            }) {
                let same = (|| -> Result<bool, String> {
                    let text: String = c
                        .query_row(
                            "SELECT value_json FROM personal_payloads WHERE id=?1",
                            [&old.payload_ref],
                            |r| r.get(0),
                        )
                        .map_err(database_error)?;
                    let value: Value = crate::memory::personal_state::decode(text)?;
                    Ok(if value["policy"] == "personal-gate-v1" {
                        value["value"] == candidate.value
                    } else {
                        value == candidate.value
                    })
                })()?;
                if same {
                    effective_at = old.effective_at;
                }
            }
        }
        let assertion = Assertion {
            id: id.clone(),
            kind: candidate.kind,
            semantic_key: candidate.semantic_key,
            payload_ref: payload.clone(),
            access,
            evidence: BTreeSet::from([chunk.source.key.clone()]),
            depends_on: BTreeSet::new(),
            input_dependencies: dependencies.clone(),
            provenance: extractor.provenance(),
            observed_at: chunk.source.recorded_at,
            effective_at,
            recorded_at: now,
            valid_from: effective_at,
            valid_until: candidate.support.valid_until,
        };
        let mut actions = vec![(id.clone(), Action::Assert)];
        actions.extend(ambiguous.into_iter().map(|old| (old, Action::Dispute)));
        if let Some(old) = candidate.replaces {
            actions.push((old, Action::Supersede { by: id.clone() }));
        }
        if consolidating {
            if let Some(old) = ledger.assertions.values().find(|a| {
                a.kind == Kind::Observation
                    && a.semantic_key == assertion.semantic_key
                    && a.access.task_request == assertion.access.task_request
                    && ledger.status(&a.id, now) == Status::Candidate
            }) {
                actions.push((old.id.clone(), Action::Supersede { by: id.clone() }));
            }
        }
        if candidate.status == Status::Active {
            actions.push((id.clone(), Action::Activate));
        }
        for (target, action) in actions {
            patch.transitions.push(Transition {
                id: crate::new_id("transition"),
                sequence: next_sequence,
                assertion_id: target,
                action,
                reason_code: "extractor-supported".into(),
                evidence: assertion.evidence.clone(),
                input_dependencies: dependencies.clone(),
                recorded_at: now,
            });
            next_sequence += 1;
        }
        let value = if candidate.kind.is_profile() || candidate.kind == Kind::Observation {
            json!({"value":candidate.value,"support":candidate.support,"origin":chunk.source.digest,"policy":"personal-gate-v1","independentOrigins":if consolidating {crate::memory::personal_state::admission::independent_origins(&current)}else{BTreeSet::new()}})
        } else {
            candidate.value
        };
        payloads.insert(payload, value);
        patch.assertions.push(assertion);
    }
    if job.finalizing {
        for old in ledger.assertions.values() {
            if ledger.status(&old.id, now) == Status::Candidate
                && old.evidence.iter().any(|k| {
                    k.id == chunk.source.key.id
                        && k.version == chunk.source.key.version
                        && *k != chunk.source.key
                })
            {
                patch.transitions.push(Transition {
                    id: crate::new_id("transition"),
                    sequence: next_sequence,
                    assertion_id: old.id.clone(),
                    action: Action::Invalidate,
                    reason_code: "full-message-finalized".into(),
                    evidence: BTreeSet::from([chunk.source.key.clone()]),
                    input_dependencies: dependencies.clone(),
                    recorded_at: now,
                });
                next_sequence += 1;
            }
        }
    }
    patch.coverage.push((
        chunk.source.key.clone(),
        if !chunk.source.finalized {
            if patch.assertions.is_empty() {
                Coverage::Pending
            } else {
                Coverage::Candidate
            }
        } else if extraction.no_change {
            Coverage::NoChange
        } else if patch
            .transitions
            .iter()
            .any(|t| t.action == Action::Activate)
        {
            Coverage::Applied
        } else {
            Coverage::Candidate
        },
    ));
    let payload_bytes = payloads
        .iter()
        .map(|(k, v)| Ok((k.clone(), crate::memory::personal_state::encode(v)?.len())))
        .collect::<Result<BTreeMap<_, _>, String>>()?;
    Ok(Built {
        patch,
        payloads,
        payload_bytes,
        dependencies,
        request_scope,
    })
}
