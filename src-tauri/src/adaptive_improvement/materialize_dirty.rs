use super::*;
/// Materialize one stable prefix. New feedback stays dirty because the upper event sequence is
/// frozen before the transaction starts; crashes roll the complete transaction back.
pub(crate) fn materialize_dirty(
    c: &Connection,
    batch_size: usize,
    now: i64,
) -> Result<Option<String>, String> {
    let upper: i64 = c
        .query_row(
            "SELECT COALESCE(MAX(cause_event_seq),0) FROM ai_dirty",
            [],
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())?;
    if upper == 0 {
        return Ok(None);
    }
    let tx = c.unchecked_transaction().map_err(|e| e.to_string())?;
    let rows = {
        let mut statement=tx.prepare("SELECT d.id,d.domain,d.scope_key,d.event_seq,d.policy_revision,d.candidate_fingerprint,d.eligible_json,d.selected,d.selection_mode,d.source_refs_json,o.technical_success,o.verifier_success,o.user_acceptance,o.correction,o.latency_ms,o.cost_micros,o.usefulness FROM ai_dirty q JOIN ai_decisions d ON d.id=q.decision_id LEFT JOIN ai_outcomes o ON o.decision_id=d.id WHERE q.cause_event_seq<=?1 ORDER BY q.cause_event_seq,d.id LIMIT ?2").map_err(|e|e.to_string())?;
        let mapped = statement
            .query_map(params![upper, batch_size as i64], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, i64>(3)?,
                    r.get::<_, i64>(4)?,
                    r.get::<_, String>(5)?,
                    r.get::<_, String>(6)?,
                    r.get::<_, String>(7)?,
                    r.get::<_, String>(8)?,
                    r.get::<_, String>(9)?,
                    r.get::<_, Option<i64>>(10)?,
                    r.get::<_, Option<i64>>(11)?,
                    r.get::<_, Option<i64>>(12)?,
                    r.get::<_, Option<i64>>(13)?,
                    r.get::<_, Option<i64>>(14)?,
                    r.get::<_, Option<i64>>(15)?,
                    r.get::<_, Option<i64>>(16)?,
                ))
            })
            .map_err(|e| e.to_string())?;
        mapped
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?
    };
    // The selected rows are from a fixed immutable event boundary. Include every source value
    // and implementation version in the digest, but never a timestamp or random ID.
    let semantic = rows
        .iter()
        .map(|row| {
            let (
                id, domain, scope, event_seq, policy_revision, fingerprint, eligible_json,
                selected, mode, sources, technical, verifier, accepted, corrected, latency, cost,
                useful,
            ) = row;
            format!(
                "{id}\u{1f}{domain}\u{1f}{scope}\u{1f}{event_seq}\u{1f}{policy_revision}\u{1f}{fingerprint}\u{1f}{eligible_json}\u{1f}{selected}\u{1f}{mode}\u{1f}{sources}\u{1f}{technical:?}\u{1f}{verifier:?}\u{1f}{accepted:?}\u{1f}{corrected:?}\u{1f}{latency:?}\u{1f}{cost:?}\u{1f}{useful:?}"
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    let dataset_id = format!(
        "ai-dataset-{}",
        &digest(format!("{upper}:{semantic}").as_bytes())[..24]
    );
    tx.execute("INSERT OR IGNORE INTO ai_datasets(id,upper_event_seq,feature_version,labeler_version,manifest_json,digest,state,created_at_ms) VALUES(?1,?2,'ai-features-v1','ai-labels-v1','{}',?3,'building',?4)",params![dataset_id,upper,digest(semantic.as_bytes()),now]).map_err(|e|e.to_string())?;
    for row in &rows {
        let (
            id,
            domain,
            scope,
            event_seq,
            policy_revision,
            fingerprint,
            eligible_json,
            selected,
            mode,
            sources,
            technical,
            verifier,
            accepted,
            corrected,
            latency,
            cost,
            useful,
        ) = row;
        let explicit = (*accepted).or(*technical).or(*verifier);
        let eligible = explicit.is_some() && corrected != &Some(1);
        let reason = if eligible {
            None
        } else if corrected == &Some(1) {
            Some("corrected")
        } else {
            Some("no_explicit_or_verified_outcome")
        };
        let features = serde_json::json!({"domain":domain,"scope":scope,"eventSeq":event_seq,"policyRevision":policy_revision,"candidateFingerprint":fingerprint,"eligibleCandidates":serde_json::from_str::<serde_json::Value>(eligible_json).unwrap_or_default(),"selected":selected,"selectionMode":mode,"sourceRefs":serde_json::from_str::<serde_json::Value>(sources).unwrap_or_default()});
        let labels = serde_json::json!({"technicalSuccess":technical,"verifierSuccess":verifier,"userAcceptance":accepted,"correction":corrected,"latencyMs":latency,"costMicros":cost,"usefulness":useful});
        tx.execute("INSERT OR IGNORE INTO ai_examples(id,dataset_id,decision_id,features_json,labels_json,eligible,exclusion_reason,group_key) VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",params![format!("aie-{}",&digest(format!("{dataset_id}:{id}").as_bytes())[..24]),dataset_id,id,features.to_string(),labels.to_string(),i64::from(eligible),reason,format!("{domain}:{scope}")]).map_err(|e|e.to_string())?;
        tx.execute(
            "DELETE FROM ai_dirty WHERE decision_id=?1 AND cause_event_seq<=?2",
            params![id, upper],
        )
        .map_err(|e| e.to_string())?;
    }
    let manifest = serde_json::json!({"upperEventSeq":upper,"featureVersion":"ai-features-v1","labelerVersion":"ai-labels-v1","exampleCount":rows.len()});
    tx.execute(
        "UPDATE ai_datasets SET manifest_json=?1,state='ready' WHERE id=?2",
        params![manifest.to_string(), dataset_id],
    )
    .map_err(|e| e.to_string())?;
    tx.commit().map_err(|e| e.to_string())?;
    Ok(Some(dataset_id))
}
/// An explicit correction is immediate, scope-bound, and supersedes (rather than mutates) its
/// predecessor. Call `revoke_override` to undo it with another auditable revision.
#[allow(clippy::too_many_arguments)] // Boundary keeps the full audited override record explicit.
pub(crate) fn set_override(
    c: &Connection,
    domain: Domain,
    scope: &str,
    candidate: &str,
    source: &str,
    revision: i64,
    expires: Option<i64>,
    now: i64,
) -> Result<(), String> {
    let tx = c.unchecked_transaction().map_err(|e| e.to_string())?;
    tx.execute("UPDATE ai_overrides SET active=0,revoked_at_ms=?1 WHERE domain=?2 AND scope_key=?3 AND active=1",params![now,domain.as_str(),scope]).map_err(|e|e.to_string())?;
    tx.execute("INSERT INTO ai_overrides(id,domain,scope_key,candidate_id,active,revision,source_id,expires_at_ms,created_at_ms) VALUES(?1,?2,?3,?4,1,?5,?6,?7,?8)",params![format!("aio-{}", digest(format!("{}:{scope}:{candidate}:{revision}",domain.as_str()).as_bytes())[..24].to_string()),domain.as_str(),scope,candidate,revision,source,expires,now]).map_err(|e|e.to_string())?;
    tx.commit().map_err(|e| e.to_string())
}
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn revoke_override(
    c: &Connection,
    domain: Domain,
    scope: &str,
    now: i64,
) -> Result<(), String> {
    c.execute("UPDATE ai_overrides SET active=0,revoked_at_ms=?1 WHERE domain=?2 AND scope_key=?3 AND active=1",params![now,domain.as_str(),scope]).map_err(|e|e.to_string()).map(|_|())
}
/// Returns only a candidate supplied by the caller. Invalidated/mismatched artifacts safely
/// fall through to rules. Overrides take precedence over learned policies.
pub(crate) fn choose(
    c: &Connection,
    domain: Domain,
    scope: &str,
    candidates: &[String],
    rules: &str,
    now: i64,
) -> Result<(String, &'static str, i64), String> {
    if !candidates.iter().any(|x| x == rules) {
        return Err("Rules candidate is not eligible".into());
    }
    let override_id: Option<(String,i64)>=c.query_row("SELECT candidate_id,revision FROM ai_overrides WHERE domain=?1 AND scope_key=?2 AND active=1 AND (expires_at_ms IS NULL OR expires_at_ms>?3) ORDER BY revision DESC LIMIT 1",params![domain.as_str(),scope,now],|r|Ok((r.get(0)?,r.get(1)?))).optional().map_err(|e|e.to_string())?;
    if let Some((candidate, revision)) = override_id {
        if candidates.contains(&candidate) {
            return Ok((candidate, "override", revision));
        }
    }
    let active: Option<(String,String,i64)>=c.query_row("SELECT a.scores_json,a.candidate_fingerprint,x.policy_revision FROM ai_activations x JOIN ai_artifacts a ON a.id=x.artifact_id WHERE x.domain=?1 AND x.scope_key=?2 AND x.active=1 AND a.state='active' ORDER BY x.policy_revision DESC LIMIT 1",params![domain.as_str(),scope],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional().map_err(|e|e.to_string())?;
    if let Some((scores_json, fingerprint, revision)) = active {
        let fp = fingerprint_for(candidates);
        if fp == fingerprint {
            let scores: std::collections::BTreeMap<String, f64> =
                serde_json::from_str(&scores_json)
                    .map_err(|_| "Invalid adaptive artifact".to_string())?;
            if let Some(best) = candidates
                .iter()
                .filter_map(|id| scores.get(id).filter(|s| s.is_finite()).map(|s| (id, s)))
                .max_by(|a, b| a.1.total_cmp(b.1).then_with(|| b.0.cmp(a.0)))
            {
                return Ok((best.0.clone(), "adaptive", revision));
            }
        }
    }
    Ok((rules.to_string(), "rules", 0))
}
pub(crate) fn fingerprint_for(candidates: &[String]) -> String {
    let mut v = candidates.to_vec();
    v.sort();
    digest(v.join("\n").as_bytes())
}
pub(crate) fn digest(input: &[u8]) -> String {
    format!("{:x}", Sha256::digest(input))
}
/// Persists an immutable scored artifact. Promotion is intentionally separate: callers must
/// supply a completed evaluation gate, rather than treating a materialized dataset as proof.
pub(crate) fn create_artifact(
    c: &Connection,
    domain: Domain,
    scope: &str,
    candidates: &[String],
    scores_json: &str,
    upper_seq: i64,
    now: i64,
) -> Result<String, String> {
    let scores: std::collections::BTreeMap<String, f64> = serde_json::from_str(scores_json)
        .map_err(|_| "Artifact scores must be a JSON object".to_string())?;
    if scores.keys().any(|id| !candidates.contains(id))
        || scores.values().any(|score| !score.is_finite())
    {
        return Err("Artifact includes an unknown or invalid candidate".into());
    }
    let fingerprint = fingerprint_for(candidates);
    let canonical = serde_json::to_string(&scores).map_err(|e| e.to_string())?;
    let content = format!(
        "{}:{scope}:{fingerprint}:{canonical}:{upper_seq}",
        domain.as_str()
    );
    let id = format!("aia-{}", &digest(content.as_bytes())[..24]);
    c.execute("INSERT OR IGNORE INTO ai_artifacts(id,domain,scope_key,candidate_fingerprint,scores_json,digest,state,source_event_upper_seq,created_at_ms) VALUES(?1,?2,?3,?4,?5,?6,'candidate',?7,?8)",params![id,domain.as_str(),scope,fingerprint,canonical,digest(content.as_bytes()),upper_seq,now]).map_err(|e|e.to_string())?;
    Ok(id)
}
/// The measured evaluation result is separate from fitting. A wide/negative interval or any
/// protocol violation keeps the artifact out of production; callers may retain it as shadow
/// telemetry but cannot activate it.
pub(crate) fn apply_evaluation_gate(
    c: &Connection,
    artifact_id: &str,
    gate: EvaluationGate,
) -> Result<&'static str, String> {
    if !gate.success_ci_lower.is_finite()
        || gate
            .resource_improvement_ci_lower
            .is_some_and(|value| !value.is_finite())
        || !gate.other_resource_regression_upper.is_finite()
    {
        return Err("Evaluation metrics must be finite".into());
    }
    let eligible = gate.examples >= 200
        && gate.recipe_examples >= 30
        && gate.independent_groups >= 20
        && gate.protocol_errors == 0
        && gate.invalid_sources == 0
        && gate.scope_leaks == 0
        && gate.unknown_candidates == 0
        && gate.success_ci_lower > 0.0
        && gate
            .resource_improvement_ci_lower
            .is_none_or(|value| value >= 0.05)
        && gate.other_resource_regression_upper <= 0.10;
    let state = if eligible { "shadow" } else { "evaluated" };
    let updated = c
        .execute(
            "UPDATE ai_artifacts SET state=?1 WHERE id=?2 AND state IN ('candidate','evaluated','shadow')",
            params![state, artifact_id],
        )
        .map_err(|e| e.to_string())?;
    if updated == 0 {
        return Err("Artifact is not available for evaluation".into());
    }
    Ok(state)
}
/// Shadow observations are recorded outside the dispatch path. Calling this is the explicit
/// reviewer boundary between a promising candidate and a policy allowed to affect a new action.
pub(crate) fn approve_shadow(c: &Connection, artifact_id: &str) -> Result<(), String> {
    let changed = c
        .execute(
            "UPDATE ai_artifacts SET state='eligible' WHERE id=?1 AND state='shadow'",
            [artifact_id],
        )
        .map_err(|e| e.to_string())?;
    if changed == 1 {
        Ok(())
    } else {
        Err("Artifact has not completed shadow evaluation".into())
    }
}
pub(crate) fn activate(
    c: &Connection,
    artifact_id: &str,
    revision: i64,
    now: i64,
) -> Result<(), String> {
    let artifact: Option<(String, String, String)> = c
        .query_row(
            "SELECT domain,scope_key,state FROM ai_artifacts WHERE id=?1",
            [artifact_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()
        .map_err(|e| e.to_string())?;
    let Some((domain, scope, state)) = artifact else {
        return Err("Unknown adaptive artifact".into());
    };
    if state != "eligible" {
        return Err("Only evaluation-eligible artifacts can be activated".into());
    }
    let tx = c.unchecked_transaction().map_err(|e| e.to_string())?;
    let current_revision: Option<i64> = tx
        .query_row(
            "SELECT policy_revision FROM ai_activations WHERE domain=?1 AND scope_key=?2 AND active=1",
            params![domain, scope],
            |row| row.get(0),
        )
        .optional()
        .map_err(|e| e.to_string())?;
    if current_revision.is_some_and(|current| revision <= current) {
        return Err("Adaptive policy revision is stale".into());
    }
    tx.execute(
        "UPDATE ai_activations SET active=0 WHERE domain=?1 AND scope_key=?2 AND active=1",
        params![domain, scope],
    )
    .map_err(|e| e.to_string())?;
    tx.execute("UPDATE ai_artifacts SET state='retired' WHERE domain=?1 AND scope_key=?2 AND state='active'",params![domain,scope]).map_err(|e|e.to_string())?;
    tx.execute(
        "UPDATE ai_artifacts SET state='active' WHERE id=?1",
        [artifact_id],
    )
    .map_err(|e| e.to_string())?;
    tx.execute("INSERT INTO ai_activations(domain,scope_key,artifact_id,policy_revision,active,activated_at_ms) VALUES(?1,?2,?3,?4,1,?5)",params![domain,scope,artifact_id,revision,now]).map_err(|e|e.to_string())?;
    tx.commit().map_err(|e| e.to_string())
}
/// Removes an active learned policy without touching a user's explicit override.  The next
/// matching dispatch therefore follows the normal override-or-rules path immediately; retired
/// evidence remains available for audit but can no longer be selected.
pub(crate) fn rollback_active_to_rules(
    c: &Connection,
    artifact_id: &str,
    now: i64,
) -> Result<(), String> {
    let tx = c
        .unchecked_transaction()
        .map_err(|error| error.to_string())?;
    let artifact: Option<(String, String, String)> = tx
        .query_row(
            "SELECT domain,scope_key,state FROM ai_artifacts WHERE id=?1",
            [artifact_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()
        .map_err(|error| error.to_string())?;
    let Some((domain, scope, state)) = artifact else {
        return Err("Unknown adaptive artifact".into());
    };
    if state != "active" {
        return Err("Only an active adaptive artifact can be returned to rules".into());
    }
    let deactivated = tx
        .execute(
            "UPDATE ai_activations SET active=0 WHERE artifact_id=?1 AND active=1",
            [artifact_id],
        )
        .map_err(|error| error.to_string())?;
    if deactivated != 1 {
        return Err("Active adaptive artifact has no current activation".into());
    }
    tx.execute(
        "UPDATE ai_artifacts SET state='retired' WHERE id=?1 AND domain=?2 AND scope_key=?3 AND state='active'",
        params![artifact_id, domain, scope],
    )
    .map_err(|error| error.to_string())?;
    tx.execute(
        "INSERT INTO ai_events(kind,source_id,payload_json,created_at_ms) VALUES('artifact_rolled_back',?1,'{}',?2)",
        params![artifact_id, now],
    )
    .map_err(|error| error.to_string())?;
    tx.commit().map_err(|error| error.to_string())
}
pub(crate) fn invalidate_source(c: &Connection, source_id: &str) -> Result<usize, String> {
    // Source references are immutable JSON at decision time. A source can be represented by
    // several domain ledgers, so an unresolved dependency is invalidated conservatively rather
    // than attempting to subtract it from learned weights.
    let changed = c
        .execute(
            "UPDATE ai_artifacts SET state='invalidated' WHERE state IN ('candidate','evaluated','shadow','eligible','active')",
            [],
        )
        .map_err(|e| e.to_string())?;
    c.execute(
        "UPDATE ai_datasets SET state='invalidated' WHERE state='ready'",
        [],
    )
    .map_err(|e| e.to_string())?;
    c.execute(
        "UPDATE ai_activations SET active=0 WHERE artifact_id IN (SELECT id FROM ai_artifacts WHERE state='invalidated')",
        [],
    )
    .map_err(|e| e.to_string())?;
    c.execute(
        "INSERT INTO ai_events(kind,source_id,payload_json,created_at_ms) VALUES('source_invalidated',?1,'{}',?2)",
        params![source_id, now_ms()],
    )
    .map_err(|e| e.to_string())?;
    Ok(changed)
}
