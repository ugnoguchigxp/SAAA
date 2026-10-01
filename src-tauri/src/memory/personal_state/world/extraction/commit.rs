//! Build and validate World ledger patches on the existing writer transaction.
use super::*;
pub(crate) fn commit(
    c: &Connection,
    extraction: &Extraction,
    source: &SourceRef,
    project: &str,
    fence: &str,
    provenance: Provenance,
    now: i64,
) -> Result<(), String> {
    commit_with_sources(c, extraction, source, &[], project, fence, provenance, now)
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn commit_with_sources(
    c: &Connection,
    extraction: &Extraction,
    source: &SourceRef,
    context_sources: &[SourceRef],
    project: &str,
    fence: &str,
    provenance: Provenance,
    now: i64,
) -> Result<(), String> {
    if extraction.no_change {
        return Ok(());
    }
    if extraction.candidates.is_empty() {
        return super::super::extracted_outcomes::commit(
            c, extraction, source, project, fence, now,
        );
    }
    let ledger = store::load(c)?;
    let patch_digest = saaa_personal_state_core::world::runtime_frame::hex_sha256(
        &serde_json::to_vec(&(project, &source.key, extraction)).map_err(|e| e.to_string())?,
    );
    let patch_id = format!("world-patch-{patch_digest}");
    if ledger.applied_patches.contains_key(&patch_id) {
        let valid:bool=c.query_row("SELECT EXISTS(SELECT 1 FROM personal_sources p JOIN personal_source_scope_refs r ON r.source_id=p.message_id AND r.version=p.version JOIN context_scopes s ON s.scope_key=r.scope_key WHERE p.message_id=?1 AND p.version=?2 AND p.available=1 AND r.scope_key=?3 AND s.state='active')",rusqlite::params![source.key.id,source.key.version,project],|r|r.get(0)).map_err(crate::database_error)?;
        return if valid {
            Ok(())
        } else {
            Err("world-extraction-source-revoked".into())
        };
    }
    let existing = load_existing_versioned(c, &ledger)?;
    let mut entities = BTreeMap::new();
    for a in ledger.assertions.values().filter(|a| {
        a.access.task_request.as_deref() == Some(project)
            && ledger.status(&a.id, now) == Status::Active
    }) {
        if let Some(p) = existing.get(&a.payload_ref) {
            if let saaa_personal_state_core::world::versioned::WorldView::Entity(v) =
                p.view(&a.semantic_key)
            {
                entities.insert(v.payload.entity_id, a.id.clone());
            }
        }
    }
    let ids: Vec<_> = extraction
        .candidates
        .iter()
        .enumerate()
        .map(|(i, _)| format!("world-{patch_digest}-{i}"))
        .collect();
    let mut decoded = Vec::new();
    for (candidate, id) in extraction.candidates.iter().zip(&ids) {
        let mut p = WorldPayloadV2::decode(&candidate.kind, &candidate.payload)?;
        if let WorldPayloadV2::Entity(e) = &p {
            if candidate.replaces.is_some() || !entities.contains_key(&e.entity_id) {
                entities.insert(e.entity_id.clone(), id.clone());
            }
        }
        if let WorldPayloadV2::Relation(r) = &mut p {
            r.basis = if candidate.epistemic
                == saaa_personal_state_core::world::extraction::EvidenceKind::Inferred
            {
                Basis::ModelHypothesis
            } else {
                Basis::UserStatement
            };
            let cited = cited_sources(candidate, source, context_sources)?;
            r.evidence_stances = cited
                .into_iter()
                .map(|source| EvidenceStance {
                    source,
                    stance: Stance::Supports,
                })
                .collect();
            saaa_personal_state_core::world::model_v2::check_relation_v2_struct(r)?;
        }
        decoded.push(p);
    }
    let mut dependencies = BTreeSet::from([source.key.clone()]);
    let mut patch = StatePatch {
        id: patch_id.clone(),
        base_revision: ledger.revision,
        input_epoch: ledger.input_epoch,
        policy_revision: ledger.policy_revision,
        fence: fence.into(),
        assertions: vec![],
        transitions: vec![],
        coverage: vec![],
    };
    let mut payloads = BTreeMap::new();
    let mut sequence = ledger.transitions.last().map_or(1, |t| t.sequence + 1);
    // Activate endpoints before dependent relations/focus, regardless of model output order.
    let mut ordered: Vec<_> = extraction.candidates.iter().zip(decoded).zip(ids).collect();
    ordered.sort_by_key(|((_, payload), _)| {
        if matches!(payload, WorldPayloadV2::Entity(_)) {
            0
        } else {
            1
        }
    });
    for ((candidate, mut p), id) in ordered {
        let mut replaces = candidate.replaces.clone();
        let mut prior_evidence = BTreeSet::new();
        let mut prior_dependencies = BTreeSet::new();
        if replaces.is_none() {
            let matching = ledger
                .assertions
                .values()
                .filter(|a| {
                    a.access.task_request.as_deref() == Some(project)
                        && ledger.status(&a.id, now) == Status::Active
                })
                .find(|a| {
                    let Some(prior) = existing.get(&a.payload_ref) else {
                        return false;
                    };
                    match (&p, prior.view(&a.semantic_key)) {
                        (
                            WorldPayloadV2::Entity(entity),
                            saaa_personal_state_core::world::versioned::WorldView::Entity(old),
                        ) => entity.entity_id == old.payload.entity_id,
                        (
                            WorldPayloadV2::Relation(relation),
                            saaa_personal_state_core::world::versioned::WorldView::Relation(old),
                        ) => {
                            identity_v2::relation_key_from_payload(project, relation, 0, None)
                                == identity_v2::relation_key_from_payload(
                                    project,
                                    &old.payload,
                                    0,
                                    None,
                                )
                        }
                        (
                            WorldPayloadV2::Focus(focus),
                            saaa_personal_state_core::world::versioned::WorldView::Focus(old),
                        ) => {
                            focus.entity_id == old.payload.entity_id
                                && focus.reason == old.payload.reason
                        }
                        _ => false,
                    }
                });
            if let Some(prior) = matching {
                // Endpoints and focus are references on repeated extraction. Their
                // payload changes require an explicit correction, never a new copy.
                if !matches!(p, WorldPayloadV2::Relation(_)) {
                    continue;
                }
                if let saaa_personal_state_core::world::versioned::WorldView::Relation(old) =
                    existing[&prior.payload_ref].view(&prior.semantic_key)
                {
                    // A different extraction patch of the same original quotes is not new support.
                    if cited_sources(candidate, source, context_sources)?.is_subset(&prior.evidence)
                    {
                        continue;
                    }
                    let mut merged = old.payload;
                    if let WorldPayloadV2::Relation(proposed) = &p {
                        for stance in &proposed.evidence_stances {
                            if !merged
                                .evidence_stances
                                .iter()
                                .any(|s| s.source == stance.source)
                            {
                                merged.evidence_stances.push(stance.clone());
                            }
                        }
                    }
                    // Payload citations are bounded representatives. The full versioned
                    // evidence set remains in Assertion.evidence/input_dependencies and
                    // personal_dependencies; forgetting any member erases the successor.
                    if merged.evidence_stances.len() > 4 {
                        let recent = merged.evidence_stances.pop().expect("nonempty evidence");
                        merged.evidence_stances.truncate(3);
                        merged.evidence_stances.push(recent);
                    }
                    p = WorldPayloadV2::Relation(merged);
                }
                replaces = Some(prior.id.clone());
                prior_evidence = prior.evidence.clone();
                prior_dependencies = prior.input_dependencies.clone();
            }
        }
        let mut depends_on = BTreeSet::new();
        let valid_from = match replaces.as_ref() {
            Some(id) => {
                ledger
                    .assertions
                    .get(id)
                    .ok_or("world-extraction-replacement")?
                    .valid_from
            }
            None => source.recorded_at,
        };
        let (kind, key) = match &p {
            WorldPayloadV2::Entity(e) => {
                if let Some(objective) = &e.objective_assertion_id {
                    depends_on.insert(objective.clone());
                }
                (
                    Kind::WorldEntity,
                    identity_v2::entity_key_v2(project, &e.entity_id),
                )
            }
            WorldPayloadV2::Relation(r) => {
                for entity in [&r.from_entity_id, &r.to_entity_id] {
                    depends_on.insert(
                        entities
                            .get(entity)
                            .ok_or("world-extraction-endpoint")?
                            .clone(),
                    );
                }
                (
                    Kind::WorldRelation,
                    identity_v2::relation_key_from_payload(project, r, valid_from, None),
                )
            }
            WorldPayloadV2::Focus(f) => {
                depends_on.insert(
                    entities
                        .get(&f.entity_id)
                        .ok_or("world-extraction-endpoint")?
                        .clone(),
                );
                if let Some(objective) = &f.objective_assertion_id {
                    depends_on.insert(objective.clone());
                }
                (
                    Kind::WorldFocus,
                    identity_v2::focus_key_v2(
                        project,
                        &f.entity_id,
                        match f.reason {
                            saaa_personal_state_core::world::FocusReason::CurrentWork => {
                                "current_work"
                            }
                            saaa_personal_state_core::world::FocusReason::ExplicitInterest => {
                                "explicit_interest"
                            }
                        },
                    ),
                )
            }
        };
        let bytes = p.canonical_bytes()?;
        let reference = payload_ref_v2(&bytes);
        let mut access = source.access.clone();
        access.task_request = Some(project.into());
        let mut local_dependencies = cited_sources(candidate, source, context_sources)?;
        local_dependencies.extend(prior_dependencies);
        for dep in &depends_on {
            if let Some(a) = ledger.assertions.get(dep) {
                if a.access.task_request.as_deref() != Some(project)
                    && !(project == format!("user:{}", ledger.principal)
                        && a.kind == Kind::Objective
                        && a.access.task_request.is_none()
                        && a.access.principal == ledger.principal)
                {
                    return Err("world-extraction-scope".into());
                }
                local_dependencies.extend(a.input_dependencies.clone());
            }
        }
        dependencies.extend(local_dependencies.clone());
        let mut evidence = cited_sources(candidate, source, context_sources)?;
        evidence.extend(prior_evidence);
        let mut actions = vec![(id.clone(), Action::Assert), (id.clone(), Action::Activate)];
        if let Some(old_id) = &replaces {
            let old = ledger
                .assertions
                .get(old_id)
                .ok_or("world-extraction-replacement")?;
            if old.observed_at > source.recorded_at {
                return Err("world-stale-observation".into());
            }
            if old.kind != kind
                || old.access.task_request.as_deref() != Some(project)
                || old.semantic_key != key
                || ledger.status(old_id, now) != Status::Active
            {
                return Err("world-extraction-replacement".into());
            }
            actions.insert(1, (old_id.clone(), Action::Supersede { by: id.clone() }));
        }
        for (target, action) in actions {
            patch.transitions.push(Transition {
                id: crate::new_id("transition"),
                sequence,
                assertion_id: target,
                action,
                reason_code: "world-source-extraction".into(),
                evidence: evidence.clone(),
                input_dependencies: local_dependencies.clone(),
                recorded_at: now,
            });
            sequence += 1;
        }
        payloads.insert(
            reference.clone(),
            serde_json::from_slice::<Value>(&bytes).map_err(|e| e.to_string())?,
        );
        patch.assertions.push(Assertion {
            id,
            kind,
            semantic_key: key,
            payload_ref: reference,
            access,
            evidence,
            depends_on,
            input_dependencies: local_dependencies,
            provenance: provenance.clone(),
            observed_at: source.recorded_at,
            effective_at: now,
            recorded_at: now,
            valid_from,
            valid_until: None,
        });
    }
    // The reducer requires each operation to inherit the full generation dependency set.
    for assertion in &mut patch.assertions {
        assertion.input_dependencies = dependencies.clone();
    }
    for transition in &mut patch.transitions {
        transition.input_dependencies = dependencies.clone();
    }
    let context = CommitContext {
        access: AccessRequest {
            principal: &ledger.principal,
            scope: &ledger.scope,
            task_request: Some(project),
            purpose: Purpose::StateExtract,
            max_classification: Classification::Confidential,
            policy_revision: ledger.policy_revision,
            authorized: true,
        },
        enabled: true,
        now,
        live_fence: fence,
        issued_patch_id: &patch_id,
        issued_assertion_ids: patch.assertions.iter().map(|a| a.id.clone()).collect(),
        issued_payload_bytes: payloads
            .iter()
            .map(|(k, v)| {
                Ok((
                    k.clone(),
                    serde_json::to_vec(v).map_err(|e| e.to_string())?.len(),
                ))
            })
            .collect::<Result<_, String>>()?,
        evidence_allowlist: dependencies.clone(),
        input_dependencies: dependencies,
    };
    store::commit(c, &patch, &context, &payloads)?;
    super::super::extracted_outcomes::commit(c, extraction, source, project, fence, now)?;
    c.execute(
        "INSERT OR IGNORE INTO personal_world_source_receipts VALUES(?1,?2,?3)",
        rusqlite::params![project, source.key.id, source.key.version],
    )
    .map_err(crate::database_error)?;
    Ok(())
}

fn cited_sources(
    candidate: &saaa_personal_state_core::world::extraction::Candidate,
    primary: &SourceRef,
    context: &[SourceRef],
) -> Result<BTreeSet<SourceKey>, String> {
    let mut cited = BTreeSet::from([primary.key.clone()]);
    for quote in &candidate.additional_quotes {
        let source = context
            .iter()
            .find(|s| s.key.id == quote.source_id && s.key.version == quote.version)
            .ok_or("world-extraction-evidence")?;
        if !source.finalized || source.role != SourceRole::User {
            return Err("world-extraction-evidence".into());
        }
        cited.insert(source.key.clone());
    }
    if cited.len() > 4 {
        return Err("world-limit".into());
    }
    Ok(cited)
}
