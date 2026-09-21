//! Conservative, local policy adaptation.
//!
//! This is deliberately a *re-ordering* layer: callers provide candidates that have already
//! passed their domain's permission and capability checks.  An adaptive policy can therefore
//! never introduce a tool, recipe, plan step, or notification channel.
use chrono::Timelike;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::sync::Arc;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Domain {
    ProviderRecipe,
    Tool,
    Plan,
    Notification,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct EvaluationGate {
    pub(crate) examples: u32,
    pub(crate) recipe_examples: u32,
    pub(crate) independent_groups: u32,
    pub(crate) protocol_errors: u32,
    pub(crate) invalid_sources: u32,
    pub(crate) scope_leaks: u32,
    pub(crate) unknown_candidates: u32,
    pub(crate) success_ci_lower: f64,
    pub(crate) resource_improvement_ci_lower: Option<f64>,
    pub(crate) other_resource_regression_upper: f64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct PairedInterval {
    pub(crate) mean: f64,
    pub(crate) lower: f64,
    pub(crate) upper: f64,
    pub(crate) groups: usize,
}

/// One offline, paired comparison. `group_key` identifies a goal/root lineage rather than an
/// individual step, so repeated steps cannot manufacture independent evidence. All values are
/// observed measurements; callers must omit an unmatched candidate/rules pair instead of
/// inventing a counterfactual result.
#[derive(Debug, Clone)]
pub(crate) struct PairedEvaluationSample {
    pub(crate) group_key: String,
    pub(crate) candidate_success: f64,
    pub(crate) rules_success: f64,
    pub(crate) candidate_resource: Option<f64>,
    pub(crate) rules_resource: Option<f64>,
    pub(crate) candidate_other_resource: Option<f64>,
    pub(crate) rules_other_resource: Option<f64>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct EvaluationSummary {
    pub(crate) success: PairedInterval,
    pub(crate) resource_improvement: Option<PairedInterval>,
    pub(crate) other_resource_regression_upper: f64,
}

/// Computes group-level paired intervals for a pre-registered evaluation pool. Resource values
/// are "lower is better"; success values are higher-is-better. Every group must provide a full
/// pair for success, while resource axes are evaluated only over their complete observed pairs.
pub(crate) fn evaluate_paired(
    samples: &[PairedEvaluationSample],
    seed: u64,
) -> Result<EvaluationSummary, String> {
    use std::collections::BTreeMap;
    let mut success = BTreeMap::<&str, Vec<f64>>::new();
    let mut resource = BTreeMap::<&str, Vec<f64>>::new();
    let mut other = BTreeMap::<&str, Vec<f64>>::new();
    for sample in samples {
        if sample.group_key.is_empty()
            || !sample.candidate_success.is_finite()
            || !sample.rules_success.is_finite()
        {
            return Err("Paired evaluation contains an invalid success observation".into());
        }
        success
            .entry(&sample.group_key)
            .or_default()
            .push(sample.candidate_success - sample.rules_success);
        match (sample.candidate_resource, sample.rules_resource) {
            (Some(candidate), Some(rules)) if candidate.is_finite() && rules.is_finite() => {
                // Positive means the candidate consumed less resource than rules.
                resource
                    .entry(&sample.group_key)
                    .or_default()
                    .push(rules - candidate);
            }
            (None, None) => {}
            _ => return Err("Paired resource observation is incomplete or non-finite".into()),
        }
        match (sample.candidate_other_resource, sample.rules_other_resource) {
            (Some(candidate), Some(rules)) if candidate.is_finite() && rules.is_finite() => {
                // Positive means a harmful regression on the other monitored resource axis.
                other
                    .entry(&sample.group_key)
                    .or_default()
                    .push(candidate - rules);
            }
            (None, None) => {}
            _ => {
                return Err("Paired other-resource observation is incomplete or non-finite".into())
            }
        }
    }
    let average = |groups: BTreeMap<&str, Vec<f64>>| {
        groups
            .into_values()
            .map(|values| values.iter().sum::<f64>() / values.len() as f64)
            .collect::<Vec<_>>()
    };
    let success = paired_bootstrap(&average(success), seed, 10_000)?;
    let resource_values = average(resource);
    let resource_improvement = if resource_values.is_empty() {
        None
    } else {
        Some(paired_bootstrap(
            &resource_values,
            seed.wrapping_add(1),
            10_000,
        )?)
    };
    let other_values = average(other);
    let other_resource_regression_upper = if other_values.is_empty() {
        0.0
    } else {
        paired_bootstrap(&other_values, seed.wrapping_add(2), 10_000)?.upper
    };
    Ok(EvaluationSummary {
        success,
        resource_improvement,
        other_resource_regression_upper,
    })
}

/// Deterministic group-level paired bootstrap used by offline evaluators. Callers provide one
/// pre-aggregated difference per independent lineage group, preventing repeated goal steps from
/// inflating confidence. It intentionally has no provider/network dependency.
pub(crate) fn paired_bootstrap(
    differences: &[f64],
    seed: u64,
    resamples: usize,
) -> Result<PairedInterval, String> {
    if differences.len() < 3
        || resamples < 100
        || differences.iter().any(|value| !value.is_finite())
    {
        return Err(
            "Paired evaluation requires at least three finite groups and 100 resamples".into(),
        );
    }
    let mean = differences.iter().sum::<f64>() / differences.len() as f64;
    let mut state = seed.max(1);
    let mut samples = Vec::with_capacity(resamples);
    for _ in 0..resamples {
        let mut total = 0.0;
        for _ in differences {
            // Numerical Recipes LCG: deterministic sampling, not production exploration.
            state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
            total += differences[((state >> 32) as usize) % differences.len()];
        }
        samples.push(total / differences.len() as f64);
    }
    samples.sort_by(f64::total_cmp);
    let lower = samples[(resamples * 25 / 1000).min(resamples - 1)];
    let upper = samples[(resamples * 975 / 1000).min(resamples - 1)];
    Ok(PairedInterval {
        mean,
        lower,
        upper,
        groups: differences.len(),
    })
}
impl Domain {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::ProviderRecipe => "provider_recipe",
            Self::Tool => "tool",
            Self::Plan => "plan",
            Self::Notification => "notification",
        }
    }

    fn parse(value: &str) -> Option<Self> {
        match value {
            "provider_recipe" => Some(Self::ProviderRecipe),
            "tool" => Some(Self::Tool),
            "plan" => Some(Self::Plan),
            "notification" => Some(Self::Notification),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DecisionObservation {
    pub(crate) id: String,
    pub(crate) domain: Domain,
    pub(crate) scope_key: String,
    pub(crate) event_seq: i64,
    pub(crate) policy_revision: i64,
    pub(crate) candidate_fingerprint: String,
    pub(crate) eligible_candidates: Vec<String>,
    pub(crate) selected: String,
    pub(crate) selection_mode: String,
    pub(crate) source_refs_json: String,
}

pub(crate) fn migrate(c: &Connection) -> rusqlite::Result<()> {
    c.execute_batch("CREATE TABLE IF NOT EXISTS ai_events (seq INTEGER PRIMARY KEY AUTOINCREMENT, kind TEXT NOT NULL, source_id TEXT NOT NULL, payload_json TEXT NOT NULL CHECK(json_valid(payload_json)), created_at_ms INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS ai_decisions (id TEXT PRIMARY KEY, domain TEXT NOT NULL CHECK(domain IN ('provider_recipe','tool','plan','notification')), scope_key TEXT NOT NULL, event_seq INTEGER NOT NULL, policy_revision INTEGER NOT NULL, candidate_fingerprint TEXT NOT NULL, eligible_json TEXT NOT NULL CHECK(json_valid(eligible_json)), selected TEXT NOT NULL, selection_mode TEXT NOT NULL CHECK(selection_mode IN ('rules','override','adaptive')), source_refs_json TEXT NOT NULL CHECK(json_valid(source_refs_json)), outcome_revision INTEGER NOT NULL DEFAULT 0, created_at_ms INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS ai_overrides (id TEXT PRIMARY KEY, domain TEXT NOT NULL CHECK(domain IN ('provider_recipe','tool','plan','notification')), scope_key TEXT NOT NULL, candidate_id TEXT NOT NULL, active INTEGER NOT NULL CHECK(active IN (0,1)), revision INTEGER NOT NULL, source_id TEXT NOT NULL, expires_at_ms INTEGER, created_at_ms INTEGER NOT NULL, revoked_at_ms INTEGER);
CREATE UNIQUE INDEX IF NOT EXISTS ai_one_active_override ON ai_overrides(domain,scope_key) WHERE active=1;
CREATE TABLE IF NOT EXISTS ai_artifacts (id TEXT PRIMARY KEY, domain TEXT NOT NULL CHECK(domain IN ('provider_recipe','tool','plan','notification')), scope_key TEXT NOT NULL, candidate_fingerprint TEXT NOT NULL, scores_json TEXT NOT NULL CHECK(json_valid(scores_json)), digest TEXT NOT NULL UNIQUE, state TEXT NOT NULL CHECK(state IN ('candidate','evaluated','shadow','eligible','active','retired','invalidated')), source_event_upper_seq INTEGER NOT NULL, created_at_ms INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS ai_activations (domain TEXT NOT NULL CHECK(domain IN ('provider_recipe','tool','plan','notification')), scope_key TEXT NOT NULL, artifact_id TEXT NOT NULL, policy_revision INTEGER NOT NULL, active INTEGER NOT NULL CHECK(active IN (0,1)), activated_at_ms INTEGER NOT NULL, PRIMARY KEY(domain,scope_key,policy_revision), FOREIGN KEY(artifact_id) REFERENCES ai_artifacts(id));
CREATE UNIQUE INDEX IF NOT EXISTS ai_one_active_activation ON ai_activations(domain,scope_key) WHERE active=1;
CREATE TABLE IF NOT EXISTS ai_outcomes (decision_id TEXT PRIMARY KEY, technical_success INTEGER, verifier_success INTEGER, user_acceptance INTEGER, correction INTEGER, latency_ms INTEGER, cost_micros INTEGER, usefulness INTEGER, source_id TEXT NOT NULL, revision INTEGER NOT NULL, created_at_ms INTEGER NOT NULL, FOREIGN KEY(decision_id) REFERENCES ai_decisions(id));
CREATE TABLE IF NOT EXISTS ai_dirty (decision_id TEXT PRIMARY KEY, cause_event_seq INTEGER NOT NULL, FOREIGN KEY(decision_id) REFERENCES ai_decisions(id) ON DELETE CASCADE);
CREATE TABLE IF NOT EXISTS ai_datasets (id TEXT PRIMARY KEY, upper_event_seq INTEGER NOT NULL, feature_version TEXT NOT NULL, labeler_version TEXT NOT NULL, manifest_json TEXT NOT NULL CHECK(json_valid(manifest_json)), digest TEXT NOT NULL UNIQUE, state TEXT NOT NULL CHECK(state IN ('building','ready','invalidated','failed')), created_at_ms INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS ai_examples (id TEXT PRIMARY KEY, dataset_id TEXT NOT NULL, decision_id TEXT NOT NULL, features_json TEXT NOT NULL CHECK(json_valid(features_json)), labels_json TEXT NOT NULL CHECK(json_valid(labels_json)), eligible INTEGER NOT NULL CHECK(eligible IN (0,1)), exclusion_reason TEXT, group_key TEXT NOT NULL, FOREIGN KEY(dataset_id) REFERENCES ai_datasets(id) ON DELETE CASCADE, FOREIGN KEY(decision_id) REFERENCES ai_decisions(id) ON DELETE CASCADE, UNIQUE(dataset_id,decision_id));")
}

/// The adaptive materializer shares the role-routing local window and idle gate. It does only
/// SQLite work; inference/evaluation stays off the conversation path and any failure is ignored
/// so a background learning fault cannot interrupt normal conversation.
pub(crate) fn start_worker(writer: Arc<crate::persistence::SqliteWriter>) {
    tauri::async_runtime::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(60));
        loop {
            interval.tick().await;
            let _ = writer.write(|connection| {
                let settings = crate::persistence::load_role_routing_settings(connection)?;
                let adaptive_domains_enabled = settings.adaptive_improvement.provider_recipe
                    || settings.adaptive_improvement.tool
                    || settings.adaptive_improvement.plan
                    || settings.adaptive_improvement.notification;
                let now = now_ms();
                let idle = connection
                    .query_row(
                        "SELECT ?1 - last_foreground_at FROM personal_scope WHERE id='primary'",
                        [now],
                        |row| row.get::<_, i64>(0),
                    )
                    .unwrap_or(0)
                    .max(0) as u32
                    / 1_000;
                let local = chrono::Local::now();
                let minutes = local.hour() as u16 * 60 + local.minute() as u16;
                if crate::role_routing::learning::scheduler::should_start(
                    &settings.learning,
                    minutes,
                    idle,
                    false,
                ) {
                    // Role-routing data has its own privacy-preserving ledger. It must not be
                    // coupled to the legacy adaptive domains being enabled.
                    let _ = crate::role_routing::learning::repository::materialize_dirty_roots(
                        connection,
                        now,
                        settings.learning.batch_size,
                    );
                    if settings.adaptive_improvement.enabled && adaptive_domains_enabled {
                        if let Some(dataset_id) = materialize_dirty(
                            connection,
                            settings.learning.batch_size as usize,
                            now,
                        )? {
                            let _ = train_candidate_artifacts(connection, &dataset_id, now);
                        }
                    }
                }
                Ok(())
            });
        }
    });
}

/// Fits a deliberately simple, domain/scope-local aggregate ranker from observed selections.
/// It never assigns a label to an unselected candidate: scores are emitted only for candidates
/// that have an explicit acceptance or verified technical/verifier outcome. The resulting
/// artifacts remain `candidate` until the separate paired evaluation gate promotes them.
pub(crate) fn train_candidate_artifacts(
    c: &Connection,
    dataset_id: &str,
    now: i64,
) -> Result<Vec<String>, String> {
    use std::collections::BTreeMap;
    type CandidateScores = BTreeMap<String, (f64, u32)>;
    let source_event_seq: i64 = c
        .query_row(
            "SELECT upper_event_seq FROM ai_datasets WHERE id=?1 AND state='ready'",
            [dataset_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(|error| error.to_string())?
        .ok_or_else(|| "Adaptive dataset is not ready".to_string())?;
    let mut groups = BTreeMap::<(String, String, String), (Vec<String>, CandidateScores)>::new();
    let mut statement = c
        .prepare(
            "SELECT features_json,labels_json FROM ai_examples WHERE dataset_id=?1 AND eligible=1",
        )
        .map_err(|error| error.to_string())?;
    let rows = statement
        .query_map([dataset_id], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(|error| error.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string())?;
    for (features, labels) in rows {
        let features: serde_json::Value =
            serde_json::from_str(&features).map_err(|_| "Invalid dataset features".to_string())?;
        let labels: serde_json::Value =
            serde_json::from_str(&labels).map_err(|_| "Invalid dataset labels".to_string())?;
        let Some(domain) = features.get("domain").and_then(serde_json::Value::as_str) else {
            continue;
        };
        let Some(scope) = features.get("scope").and_then(serde_json::Value::as_str) else {
            continue;
        };
        let Some(selected) = features.get("selected").and_then(serde_json::Value::as_str) else {
            continue;
        };
        let candidates = features
            .get("eligibleCandidates")
            .and_then(serde_json::Value::as_array)
            .map(|values| {
                values
                    .iter()
                    .filter_map(|value| value.as_str().map(str::to_owned))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        if candidates.is_empty() || !candidates.iter().any(|candidate| candidate == selected) {
            continue;
        }
        let outcome = ["userAcceptance", "verifierSuccess", "technicalSuccess"]
            .into_iter()
            .find_map(|key| labels.get(key).and_then(json_bool));
        let Some(outcome) = outcome else { continue };
        let fingerprint = fingerprint_for(&candidates);
        let (_, scores) = groups
            .entry((domain.to_owned(), scope.to_owned(), fingerprint))
            .or_insert_with(|| (candidates, BTreeMap::new()));
        let entry = scores.entry(selected.to_owned()).or_insert((0.0, 0));
        entry.0 += if outcome { 1.0 } else { 0.0 };
        entry.1 += 1;
    }
    let mut artifacts = Vec::new();
    for ((domain, scope, fingerprint), (candidates, scores)) in groups {
        let Some(domain) = Domain::parse(&domain) else {
            continue;
        };
        if candidates.len() < 2 || scores.is_empty() {
            continue;
        }
        let scores = scores
            .into_iter()
            .map(|(candidate, (sum, count))| (candidate, sum / count as f64))
            .collect::<BTreeMap<_, _>>();
        let score_json = serde_json::to_string(&scores).map_err(|error| error.to_string())?;
        // Recompute the fingerprint from the candidates and reject corrupt examples rather than
        // training an artifact whose dispatch set does not match its evidence.
        if fingerprint != fingerprint_for(&candidates) {
            continue;
        }
        artifacts.push(create_artifact(
            c,
            domain,
            &scope,
            &candidates,
            &score_json,
            source_event_seq,
            now,
        )?);
    }
    Ok(artifacts)
}

fn json_bool(value: &serde_json::Value) -> Option<bool> {
    value.as_bool().or_else(|| {
        value.as_i64().and_then(|value| match value {
            0 => Some(false),
            1 => Some(true),
            _ => None,
        })
    })
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or(0)
}

pub(crate) fn record_decision(
    c: &Connection,
    d: &DecisionObservation,
    now: i64,
) -> Result<(), String> {
    if d.eligible_candidates.is_empty()
        || !d.eligible_candidates.contains(&d.selected)
        || d.event_seq < 0
    {
        return Err("Adaptive decision must select one already-eligible candidate".into());
    }
    c.execute(
        "INSERT INTO ai_events(kind,source_id,payload_json,created_at_ms) VALUES('decision',?1,'{}',?2)",
        params![d.id, now],
    )
    .map_err(|e| e.to_string())?;
    let captured_event_seq = c.last_insert_rowid();
    c.execute("INSERT INTO ai_decisions(id,domain,scope_key,event_seq,policy_revision,candidate_fingerprint,eligible_json,selected,selection_mode,source_refs_json,created_at_ms) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)", params![d.id,d.domain.as_str(),d.scope_key,captured_event_seq,d.policy_revision,d.candidate_fingerprint,serde_json::to_string(&d.eligible_candidates).map_err(|e|e.to_string())?,d.selected,d.selection_mode,d.source_refs_json,now]).map_err(|e|e.to_string())?;
    Ok(())
}

pub(crate) fn record_outcome(
    c: &Connection,
    decision_id: &str,
    technical_success: Option<bool>,
    verifier_success: Option<bool>,
    user_acceptance: Option<bool>,
    correction: Option<bool>,
    latency_ms: Option<i64>,
    cost_micros: Option<i64>,
    usefulness: Option<bool>,
    source_id: &str,
    revision: i64,
    now: i64,
) -> Result<(), String> {
    if latency_ms.is_some_and(|value| value < 0) || cost_micros.is_some_and(|value| value < 0) {
        return Err("Outcome resources cannot be negative".into());
    }
    let tx = c.unchecked_transaction().map_err(|e| e.to_string())?;
    record_outcome_in_transaction(
        &tx,
        decision_id,
        technical_success,
        verifier_success,
        user_acceptance,
        correction,
        latency_ms,
        cost_micros,
        usefulness,
        source_id,
        revision,
        now,
    )?;
    tx.commit().map_err(|e| e.to_string())
}

pub(crate) fn record_outcome_in_transaction(
    c: &Connection,
    decision_id: &str,
    technical_success: Option<bool>,
    verifier_success: Option<bool>,
    user_acceptance: Option<bool>,
    correction: Option<bool>,
    latency_ms: Option<i64>,
    cost_micros: Option<i64>,
    usefulness: Option<bool>,
    source_id: &str,
    revision: i64,
    now: i64,
) -> Result<(), String> {
    if latency_ms.is_some_and(|value| value < 0) || cost_micros.is_some_and(|value| value < 0) {
        return Err("Outcome resources cannot be negative".into());
    }
    c.execute("INSERT INTO ai_events(kind,source_id,payload_json,created_at_ms) VALUES('outcome',?1,'{}',?2)",params![source_id,now]).map_err(|e|e.to_string())?;
    let event_seq = c.last_insert_rowid();
    c.execute("INSERT INTO ai_outcomes(decision_id,technical_success,verifier_success,user_acceptance,correction,latency_ms,cost_micros,usefulness,source_id,revision,created_at_ms) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11) ON CONFLICT(decision_id) DO UPDATE SET technical_success=excluded.technical_success,verifier_success=excluded.verifier_success,user_acceptance=excluded.user_acceptance,correction=excluded.correction,latency_ms=excluded.latency_ms,cost_micros=excluded.cost_micros,usefulness=excluded.usefulness,source_id=excluded.source_id,revision=excluded.revision,created_at_ms=excluded.created_at_ms",params![decision_id,technical_success.map(i64::from),verifier_success.map(i64::from),user_acceptance.map(i64::from),correction.map(i64::from),latency_ms,cost_micros,usefulness.map(i64::from),source_id,revision,now]).map_err(|e|e.to_string())?;
    c.execute("INSERT INTO ai_dirty(decision_id,cause_event_seq) VALUES(?1,?2) ON CONFLICT(decision_id) DO UPDATE SET cause_event_seq=MAX(cause_event_seq,excluded.cause_event_seq)",params![decision_id,event_seq])
        .map(|_| ())
        .map_err(|e| e.to_string())
}

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

#[cfg(test)]
mod tests {
    use super::*;

    fn decision(id: &str) -> DecisionObservation {
        DecisionObservation {
            id: id.into(),
            domain: Domain::Plan,
            scope_key: "project-a".into(),
            event_seq: 1,
            policy_revision: 1,
            candidate_fingerprint: fingerprint_for(&["a".into(), "b".into()]),
            eligible_candidates: vec!["a".into(), "b".into()],
            selected: "a".into(),
            selection_mode: "rules".into(),
            source_refs_json: "[]".into(),
        }
    }
    #[test]
    fn ai_02_scope_override_changes_only_next_matching_choice() {
        let c = Connection::open_in_memory().unwrap();
        migrate(&c).unwrap();
        let candidates = vec!["a".into(), "b".into()];
        set_override(&c, Domain::Tool, "project-a", "b", "feedback", 1, None, 1).unwrap();
        assert_eq!(
            choose(&c, Domain::Tool, "project-a", &candidates, "a", 2)
                .unwrap()
                .0,
            "b"
        );
        assert_eq!(
            choose(&c, Domain::Tool, "project-b", &candidates, "a", 2)
                .unwrap()
                .0,
            "a"
        );
        revoke_override(&c, Domain::Tool, "project-a", 3).unwrap();
        assert_eq!(
            choose(&c, Domain::Tool, "project-a", &candidates, "a", 4)
                .unwrap()
                .0,
            "a"
        );
    }
    #[test]
    fn ai_07_ineligible_override_and_artifact_fall_back_to_rules() {
        let c = Connection::open_in_memory().unwrap();
        migrate(&c).unwrap();
        let candidates = vec!["a".into()];
        set_override(&c, Domain::Plan, "p", "b", "f", 1, None, 1).unwrap();
        assert_eq!(
            choose(&c, Domain::Plan, "p", &candidates, "a", 2)
                .unwrap()
                .0,
            "a"
        );
    }
    #[test]
    fn ai_07_only_eligible_artifact_can_change_dispatch() {
        let c = Connection::open_in_memory().unwrap();
        migrate(&c).unwrap();
        let candidates = vec!["a".into(), "b".into()];
        let artifact = create_artifact(
            &c,
            Domain::ProviderRecipe,
            "respond",
            &candidates,
            r#"{"a":0.1,"b":0.9}"#,
            0,
            1,
        )
        .unwrap();
        assert!(activate(&c, &artifact, 1, 2).is_err());
        assert_eq!(
            apply_evaluation_gate(
                &c,
                &artifact,
                EvaluationGate {
                    examples: 200,
                    recipe_examples: 30,
                    independent_groups: 20,
                    protocol_errors: 0,
                    invalid_sources: 0,
                    scope_leaks: 0,
                    unknown_candidates: 0,
                    success_ci_lower: 0.01,
                    resource_improvement_ci_lower: None,
                    other_resource_regression_upper: 0.0,
                },
            )
            .unwrap(),
            "shadow"
        );
        approve_shadow(&c, &artifact).unwrap();
        activate(&c, &artifact, 1, 2).unwrap();
        assert_eq!(
            choose(&c, Domain::ProviderRecipe, "respond", &candidates, "a", 3)
                .unwrap()
                .0,
            "b"
        );
    }

    #[test]
    fn ai_13_rollback_active_artifact_returns_matching_scope_to_rules() {
        let c = Connection::open_in_memory().unwrap();
        migrate(&c).unwrap();
        let candidates = vec!["a".into(), "b".into()];
        let artifact = create_artifact(
            &c,
            Domain::Tool,
            "project-a",
            &candidates,
            r#"{"a":0.1,"b":0.9}"#,
            0,
            1,
        )
        .unwrap();
        c.execute(
            "UPDATE ai_artifacts SET state='shadow' WHERE id=?1",
            [&artifact],
        )
        .unwrap();
        approve_shadow(&c, &artifact).unwrap();
        activate(&c, &artifact, 1, 2).unwrap();
        rollback_active_to_rules(&c, &artifact, 3).unwrap();
        assert_eq!(
            choose(&c, Domain::Tool, "project-a", &candidates, "a", 4)
                .unwrap()
                .0,
            "a"
        );
        let state: String = c
            .query_row(
                "SELECT state FROM ai_artifacts WHERE id=?1",
                [&artifact],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(state, "retired");
    }

    #[test]
    fn ai_06_insufficient_data_cannot_be_promoted() {
        let c = Connection::open_in_memory().unwrap();
        migrate(&c).unwrap();
        let candidates = vec!["a".into()];
        let artifact = create_artifact(
            &c,
            Domain::Tool,
            "project-a",
            &candidates,
            r#"{"a":1.0}"#,
            0,
            1,
        )
        .unwrap();
        assert_eq!(
            apply_evaluation_gate(
                &c,
                &artifact,
                EvaluationGate {
                    examples: 199,
                    recipe_examples: 30,
                    independent_groups: 20,
                    protocol_errors: 0,
                    invalid_sources: 0,
                    scope_leaks: 0,
                    unknown_candidates: 0,
                    success_ci_lower: 0.01,
                    resource_improvement_ci_lower: None,
                    other_resource_regression_upper: 0.0,
                },
            )
            .unwrap(),
            "evaluated"
        );
        assert!(activate(&c, &artifact, 1, 2).is_err());
    }

    #[test]
    fn ai_12_source_invalidation_deactivates_policy_and_falls_back() {
        let c = Connection::open_in_memory().unwrap();
        migrate(&c).unwrap();
        let candidates = vec!["a".into(), "b".into()];
        let artifact = create_artifact(
            &c,
            Domain::Tool,
            "project-a",
            &candidates,
            r#"{"a":0.1,"b":0.9}"#,
            0,
            1,
        )
        .unwrap();
        c.execute(
            "UPDATE ai_artifacts SET state='shadow' WHERE id=?1",
            [&artifact],
        )
        .unwrap();
        approve_shadow(&c, &artifact).unwrap();
        activate(&c, &artifact, 1, 2).unwrap();
        assert_eq!(
            choose(&c, Domain::Tool, "project-a", &candidates, "a", 3)
                .unwrap()
                .0,
            "b"
        );
        invalidate_source(&c, "forgotten-message").unwrap();
        assert_eq!(
            choose(&c, Domain::Tool, "project-a", &candidates, "a", 4)
                .unwrap()
                .0,
            "a"
        );
    }

    #[test]
    fn ai_12_candidate_version_change_falls_back_without_retiring_the_artifact() {
        let c = Connection::open_in_memory().unwrap();
        migrate(&c).unwrap();
        let original_candidates = vec!["a".into(), "b".into()];
        let artifact = create_artifact(
            &c,
            Domain::Tool,
            "project-a",
            &original_candidates,
            r#"{"a":0.1,"b":0.9}"#,
            0,
            1,
        )
        .unwrap();
        c.execute(
            "UPDATE ai_artifacts SET state='shadow' WHERE id=?1",
            [&artifact],
        )
        .unwrap();
        approve_shadow(&c, &artifact).unwrap();
        activate(&c, &artifact, 1, 2).unwrap();
        let updated_candidates = vec!["a".into(), "c".into()];
        assert_eq!(
            choose(&c, Domain::Tool, "project-a", &updated_candidates, "a", 3)
                .unwrap()
                .0,
            "a"
        );
        let state: String = c
            .query_row(
                "SELECT state FROM ai_artifacts WHERE id=?1",
                [&artifact],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(state, "active");
    }

    #[test]
    fn ai_06_paired_bootstrap_is_grouped_and_reproducible() {
        let differences = [0.1, 0.2, -0.1, 0.3];
        let first = paired_bootstrap(&differences, 42, 10_000).unwrap();
        let again = paired_bootstrap(&differences, 42, 10_000).unwrap();
        assert_eq!(first, again);
        assert_eq!(first.groups, 4);
        assert!(first.lower <= first.mean && first.mean <= first.upper);
    }

    #[test]
    fn ai_06_paired_runner_groups_steps_and_rejects_missing_pairs() {
        let samples = vec![
            PairedEvaluationSample {
                group_key: "g1".into(),
                candidate_success: 1.0,
                rules_success: 0.0,
                candidate_resource: Some(8.0),
                rules_resource: Some(10.0),
                candidate_other_resource: Some(1.0),
                rules_other_resource: Some(1.0),
            },
            PairedEvaluationSample {
                group_key: "g1".into(),
                candidate_success: 0.0,
                rules_success: 0.0,
                candidate_resource: Some(8.0),
                rules_resource: Some(10.0),
                candidate_other_resource: Some(1.0),
                rules_other_resource: Some(1.0),
            },
            PairedEvaluationSample {
                group_key: "g2".into(),
                candidate_success: 1.0,
                rules_success: 0.0,
                candidate_resource: Some(7.0),
                rules_resource: Some(10.0),
                candidate_other_resource: Some(1.0),
                rules_other_resource: Some(1.0),
            },
            PairedEvaluationSample {
                group_key: "g3".into(),
                candidate_success: 1.0,
                rules_success: 1.0,
                candidate_resource: Some(9.0),
                rules_resource: Some(10.0),
                candidate_other_resource: Some(1.0),
                rules_other_resource: Some(1.0),
            },
        ];
        let summary = evaluate_paired(&samples, 7).unwrap();
        assert_eq!(summary.success.groups, 3);
        assert_eq!(summary.resource_improvement.unwrap().groups, 3);
        let mut incomplete = samples;
        incomplete[0].rules_resource = None;
        assert!(evaluate_paired(&incomplete, 7).is_err());
    }

    #[test]
    fn ai_03_materialization_snapshots_outcomes_and_keeps_silence_unknown() {
        let c = Connection::open_in_memory().unwrap();
        migrate(&c).unwrap();
        record_decision(&c, &decision("silent"), 1).unwrap();
        let event_seq: i64 = c
            .query_row(
                "SELECT event_seq FROM ai_decisions WHERE id='silent'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(event_seq > 0);
        record_outcome(
            &c, "silent", None, None, None, None, None, None, None, "source-a", 1, 2,
        )
        .unwrap();
        let dataset = materialize_dirty(&c, 10, 3).unwrap().unwrap();
        let row: (i64, String) = c
            .query_row(
                "SELECT eligible, exclusion_reason FROM ai_examples WHERE dataset_id=?1",
                [&dataset],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(row, (0, "no_explicit_or_verified_outcome".into()));
        assert!(materialize_dirty(&c, 10, 4).unwrap().is_none());
    }

    #[test]
    fn ai_05_trainer_scores_only_observed_selected_candidates() {
        let c = Connection::open_in_memory().unwrap();
        migrate(&c).unwrap();
        let mut first = decision("observed-a");
        first.domain = Domain::Tool;
        first.scope_key = "project-a".into();
        first.eligible_candidates = vec!["a".into(), "b".into()];
        first.selected = "a".into();
        first.candidate_fingerprint = fingerprint_for(&first.eligible_candidates);
        record_decision(&c, &first, 1).unwrap();
        record_outcome(
            &c,
            "observed-a",
            Some(true),
            None,
            None,
            None,
            None,
            None,
            None,
            "source",
            1,
            2,
        )
        .unwrap();
        let dataset = materialize_dirty(&c, 10, 3).unwrap().unwrap();
        let artifacts = train_candidate_artifacts(&c, &dataset, 4).unwrap();
        assert_eq!(artifacts.len(), 1);
        let scores: String = c
            .query_row(
                "SELECT scores_json FROM ai_artifacts WHERE id=?1",
                [&artifacts[0]],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(scores, r#"{"a":1.0}"#);
    }
}
