use super::*;
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
CREATE TABLE IF NOT EXISTS ai_examples (id TEXT PRIMARY KEY, dataset_id TEXT NOT NULL, decision_id TEXT NOT NULL, features_json TEXT NOT NULL CHECK(json_valid(features_json)), labels_json TEXT NOT NULL CHECK(json_valid(labels_json)), eligible INTEGER NOT NULL CHECK(eligible IN (0,1)), exclusion_reason TEXT, group_key TEXT NOT NULL, FOREIGN KEY(dataset_id) REFERENCES ai_datasets(id) ON DELETE CASCADE, FOREIGN KEY(decision_id) REFERENCES ai_decisions(id) ON DELETE CASCADE, UNIQUE(dataset_id,decision_id));
CREATE TABLE IF NOT EXISTS ai_evaluation_records (id TEXT PRIMARY KEY, artifact_id TEXT NOT NULL, dataset_digest TEXT NOT NULL, artifact_digest TEXT NOT NULL, domain TEXT NOT NULL, scope_key TEXT NOT NULL, candidate_fingerprint TEXT NOT NULL, evaluator_version TEXT NOT NULL, seed INTEGER NOT NULL, group_split_json TEXT NOT NULL CHECK(json_valid(group_split_json)), summary_json TEXT NOT NULL CHECK(json_valid(summary_json)), source_kind TEXT NOT NULL, created_at_ms INTEGER NOT NULL, UNIQUE(artifact_id, dataset_digest, evaluator_version, seed), FOREIGN KEY(artifact_id) REFERENCES ai_artifacts(id));
CREATE TABLE IF NOT EXISTS ai_evaluation_receipts (id TEXT PRIMARY KEY, artifact_id TEXT NOT NULL, kind TEXT NOT NULL CHECK(kind IN ('approve','activate','rollback')), expected_revision INTEGER, created_at_ms INTEGER NOT NULL);")
}
/// The adaptive materializer shares the role-routing local window and idle gate. It does only
/// SQLite work; inference/evaluation stays off the conversation path and any failure is ignored
/// so a background learning fault cannot interrupt normal conversation.
pub(crate) fn start_worker(
    writer: Arc<crate::persistence::SqliteWriter>,
    data_directory: std::path::PathBuf,
) {
    tauri::async_runtime::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(60));
        loop {
            interval.tick().await;
            let cleanup_now = now_ms();
            let _ = crate::role_routing::learning::invalidation::cleanup_invalidated_exports_with_writer(
                &writer,
                &data_directory.join("role-routing-learning"),
                cleanup_now,
            );
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
                let day_key = format!("{:04}-{:02}-{:02}", local.year(), local.month(), local.day());
                let already_running: bool = connection.query_row(
                    "SELECT EXISTS(SELECT 1 FROM rr_learning_runs WHERE status='running')",
                    [],
                    |row| row.get(0),
                ).unwrap_or(false);
                let foreground_active: bool = connection.query_row(
                    "SELECT EXISTS(SELECT 1 FROM runtime_runs WHERE status='running')",
                    [],
                    |row| row.get(0),
                ).unwrap_or(true);
                let last_completed_day: Option<String> = connection.query_row(
                    "SELECT MAX(day_key) FROM rr_learning_runs WHERE status='completed'",
                    [],
                    |row| row.get(0),
                ).unwrap_or(None);
                if let Some(reason) = crate::role_routing::learning::scheduler::should_start_daily(
                    &settings.learning,
                    minutes,
                    idle,
                    already_running,
                    foreground_active,
                    &day_key,
                    last_completed_day.as_deref(),
                ) {
                    let reason = match reason {
                        crate::role_routing::learning::scheduler::StartReason::Window => "window",
                        crate::role_routing::learning::scheduler::StartReason::MissedWindow => "missed_window",
                    };
                    let resumed = connection.execute(
                        "UPDATE rr_learning_runs SET status='running',error_code=NULL WHERE day_key=?1 AND status='paused'",
                        [&day_key],
                    ).map_err(|error| error.to_string())? == 1;
                    let claimed = resumed || connection.execute(
                        "INSERT OR IGNORE INTO rr_learning_runs(day_key,status,reason,started_at_ms) VALUES(?1,'running',?2,?3)",
                        params![day_key,reason,now],
                    ).map_err(|error| error.to_string())? == 1;
                    if !claimed {
                        return Ok(());
                    }
                    // Role-routing data has its own privacy-preserving ledger. It must not be
                    // coupled to the legacy adaptive domains being enabled.
                    let materialized = crate::role_routing::learning::repository::materialize_dirty_roots(
                        connection,
                        now,
                        settings.learning.batch_size,
                    );
                    let materialized = match materialized {
                        Ok(dataset) => dataset,
                        Err(_) => {
                            connection.execute("UPDATE rr_learning_runs SET status='failed',error_code='materialize_failed',completed_at_ms=?1 WHERE day_key=?2 AND status='running'", params![now,day_key]).map_err(|error| error.to_string())?;
                            return Ok(());
                        }
                    };
                    if settings.adaptive_improvement.enabled && adaptive_domains_enabled {
                        if let Some(dataset_id) = materialize_dirty(
                            connection,
                            settings.learning.batch_size as usize,
                            now,
                        )? {
                            let _ = train_candidate_artifacts(connection, &dataset_id, now);
                        }
                    }
                    let more_pages: bool = connection.query_row("SELECT EXISTS(SELECT 1 FROM rr_learning_dirty)", [], |row| row.get(0)).map_err(|error| error.to_string())?;
                    let next_status = if materialized.is_some() && more_pages { "paused" } else { "completed" };
                    connection.execute("UPDATE rr_learning_runs SET status=?1,completed_at_ms=CASE WHEN ?1='completed' THEN ?2 ELSE NULL END,error_code=NULL WHERE day_key=?3 AND status='running'", params![next_status,now,day_key]).map_err(|error| error.to_string())?;
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
pub(super) fn json_bool(value: &serde_json::Value) -> Option<bool> {
    value.as_bool().or_else(|| {
        value.as_i64().and_then(|value| match value {
            0 => Some(false),
            1 => Some(true),
            _ => None,
        })
    })
}
pub(super) fn now_ms() -> i64 {
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
#[allow(clippy::too_many_arguments)] // Mirrors the persisted outcome columns atomically.
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
#[allow(clippy::too_many_arguments)] // Transactional twin of `record_outcome`.
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
