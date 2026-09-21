//! Host-owned evaluation records. UI scores cannot promote an artifact.
use crate::adaptive_improvement::{
    apply_evaluation_gate, evaluate_paired, EvaluationGate, PairedEvaluationSample,
};
use crate::{database_error, new_id};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use ts_rs::TS;

pub(crate) const EVALUATOR_VERSION: &str = "rf5-eval-1";
const SHADOW_MIN_JUDGMENTS: i64 = 30;

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub(crate) struct EvaluationPair {
    pub group_key: String,
    pub split: String,
    pub candidate_success: f64,
    pub rules_success: f64,
    pub candidate_resource: Option<f64>,
    pub rules_resource: Option<f64>,
    pub candidate_other_resource: Option<f64>,
    pub rules_other_resource: Option<f64>,
    pub evidence_digest: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub(crate) struct EvaluationBundle {
    pub artifact_id: String,
    pub dataset_digest: String,
    pub seed: u64,
    pub pairs: Vec<EvaluationPair>,
}

#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
pub(crate) struct EvaluationView {
    pub artifact_id: String,
    pub domain: String,
    pub scope_key: String,
    pub state: String,
    pub stop_reason: Option<String>,
    pub examples: i64,
    pub groups: i64,
    pub success_lower: Option<f64>,
    pub policy_revision: Option<i64>,
}

pub(crate) fn resource_ratio(candidate: f64, rules: f64) -> Result<f64, String> {
    if !candidate.is_finite() || !rules.is_finite() {
        return Err("resource_non_finite".into());
    }
    if rules == 0.0 {
        return Err("resource_baseline_zero".into());
    }
    Ok((rules - candidate) / rules.abs())
}

pub(crate) fn import_bundle(
    connection: &Connection,
    bundle: &EvaluationBundle,
    now: i64,
) -> Result<&'static str, String> {
    if bundle.pairs.is_empty() {
        return Err("evaluation_bundle_empty".into());
    }
    let artifact: Option<(String, String, String, String)> = connection
        .query_row(
            "SELECT digest,domain,scope_key,candidate_fingerprint FROM ai_artifacts WHERE id=?1",
            [&bundle.artifact_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .optional()
        .map_err(database_error)?;
    let Some((artifact_digest, domain, scope, fingerprint)) = artifact else {
        return Err("unknown_artifact".into());
    };
    let mut samples = Vec::new();
    let mut train_groups = std::collections::BTreeSet::new();
    let mut eval_groups = std::collections::BTreeSet::new();
    for pair in &bundle.pairs {
        if pair.evidence_digest.is_empty() || pair.group_key.is_empty() {
            return Err("pair_unobserved".into());
        }
        if pair.split == "train" {
            train_groups.insert(pair.group_key.clone());
            continue;
        }
        if pair.split != "eval" {
            return Err("unknown_split".into());
        }
        eval_groups.insert(pair.group_key.clone());
        let candidate_resource = match (pair.candidate_resource, pair.rules_resource) {
            (Some(candidate), Some(rules)) => {
                let _ = resource_ratio(candidate, rules)?;
                Some((candidate / rules.abs(), 1.0))
            }
            (None, None) => None,
            _ => return Err("pair_unobserved".into()),
        };
        let candidate_other = match (pair.candidate_other_resource, pair.rules_other_resource) {
            (Some(candidate), Some(rules)) => {
                if rules == 0.0 || !candidate.is_finite() || !rules.is_finite() {
                    return Err("resource_baseline_zero".into());
                }
                Some((candidate / rules.abs(), 1.0))
            }
            (None, None) => None,
            _ => return Err("pair_unobserved".into()),
        };
        samples.push(PairedEvaluationSample {
            group_key: pair.group_key.clone(),
            candidate_success: pair.candidate_success,
            rules_success: pair.rules_success,
            candidate_resource: candidate_resource.map(|v| v.0),
            rules_resource: candidate_resource.map(|v| v.1),
            candidate_other_resource: candidate_other.map(|v| v.0),
            rules_other_resource: candidate_other.map(|v| v.1),
        });
    }
    if !train_groups.is_disjoint(&eval_groups) {
        return Err("train_eval_group_overlap".into());
    }
    if samples.is_empty() {
        return Err("eval_split_empty".into());
    }
    let summary = evaluate_paired(&samples, bundle.seed)?;
    let record_id = format!(
        "aievr-{}",
        &digest(format!(
            "{}:{}:{}:{}",
            bundle.artifact_id, bundle.dataset_digest, EVALUATOR_VERSION, bundle.seed
        ))[..24]
    );
    let inserted = connection
        .execute(
            "INSERT OR IGNORE INTO ai_evaluation_records(
               id,artifact_id,dataset_digest,artifact_digest,domain,scope_key,candidate_fingerprint,
               evaluator_version,seed,group_split_json,summary_json,source_kind,created_at_ms)
             VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,'host_bundle',?12)",
            params![
                record_id,
                bundle.artifact_id,
                bundle.dataset_digest,
                artifact_digest,
                domain,
                scope,
                fingerprint,
                EVALUATOR_VERSION,
                bundle.seed as i64,
                serde_json::json!({"eval": eval_groups, "train": train_groups}).to_string(),
                serde_json::to_string(&serde_json::json!({
                    "successLower": summary.success.lower,
                    "resourceLower": summary.resource_improvement.map(|v| v.lower),
                    "otherUpper": summary.other_resource_regression_upper,
                    "groups": summary.success.groups,
                    "examples": samples.len(),
                }))
                .map_err(|e| e.to_string())?,
                now
            ],
        )
        .map_err(database_error)?;
    if inserted == 0 {
        return Err("duplicate_evaluation".into());
    }
    apply_evaluation_gate(
        connection,
        &bundle.artifact_id,
        EvaluationGate {
            examples: samples.len() as u32,
            recipe_examples: samples.len() as u32,
            independent_groups: summary.success.groups as u32,
            protocol_errors: 0,
            invalid_sources: 0,
            scope_leaks: 0,
            unknown_candidates: 0,
            success_ci_lower: summary.success.lower,
            resource_improvement_ci_lower: summary.resource_improvement.map(|v| v.lower),
            other_resource_regression_upper: summary.other_resource_regression_upper,
        },
    )
}

pub(crate) fn list_views(connection: &Connection) -> Result<Vec<EvaluationView>, String> {
    let mut statement = connection
        .prepare(
            "SELECT a.id,a.domain,a.scope_key,a.state,
                    (SELECT x.policy_revision FROM ai_activations x WHERE x.artifact_id=a.id AND x.active=1),
                    (SELECT json_extract(r.summary_json,'$.groups') FROM ai_evaluation_records r WHERE r.artifact_id=a.id ORDER BY r.created_at_ms DESC LIMIT 1),
                    (SELECT json_extract(r.summary_json,'$.successLower') FROM ai_evaluation_records r WHERE r.artifact_id=a.id ORDER BY r.created_at_ms DESC LIMIT 1),
                    (SELECT json_extract(r.summary_json,'$.examples') FROM ai_evaluation_records r WHERE r.artifact_id=a.id ORDER BY r.created_at_ms DESC LIMIT 1)
             FROM ai_artifacts a
             ORDER BY a.created_at_ms DESC LIMIT 48",
        )
        .map_err(database_error)?;
    let rows = statement
        .query_map([], |row| {
            let state: String = row.get(3)?;
            Ok(EvaluationView {
                artifact_id: row.get(0)?,
                domain: row.get(1)?,
                scope_key: row.get(2)?,
                policy_revision: row.get(4)?,
                groups: row.get::<_, Option<i64>>(5)?.unwrap_or(0),
                success_lower: row.get(6)?,
                examples: row.get::<_, Option<i64>>(7)?.unwrap_or(0),
                stop_reason: match state.as_str() {
                    "evaluated" => Some("evaluation_threshold".into()),
                    "candidate" => Some("no_evaluation_record".into()),
                    "shadow" => Some("shadow_observations".into()),
                    _ => None,
                },
                state,
            })
        })
        .map_err(database_error)?;
    rows.collect::<Result<Vec<_>, _>>().map_err(database_error)
}

pub(crate) fn approve(
    connection: &Connection,
    artifact_id: &str,
    expected_revision: Option<i64>,
    now: i64,
) -> Result<(), String> {
    let (domain, scope, fingerprint, state): (String, String, String, String) = connection
        .query_row(
            "SELECT domain,scope_key,candidate_fingerprint,state FROM ai_artifacts WHERE id=?1",
            [artifact_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .map_err(|_| "unknown_artifact".to_string())?;
    if state != "shadow" {
        return Err("not_shadow".into());
    }
    let shadow_started: i64 = connection
        .query_row(
            "SELECT COALESCE(MAX(created_at_ms),0) FROM ai_evaluation_records WHERE artifact_id=?1",
            [artifact_id],
            |row| row.get(0),
        )
        .map_err(database_error)?;
    let judgments: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM ai_decisions d
             JOIN ai_outcomes o ON o.decision_id=d.id
             WHERE d.domain=?1 AND d.scope_key=?2 AND d.candidate_fingerprint=?3
               AND d.created_at_ms>=?4
               AND d.selection_mode IN ('rules','override')",
            params![domain, scope, fingerprint, shadow_started],
            |row| row.get(0),
        )
        .map_err(database_error)?;
    if judgments < SHADOW_MIN_JUDGMENTS {
        return Err("shadow_observations_insufficient".into());
    }
    crate::adaptive_improvement::approve_shadow(connection, artifact_id)?;
    receipt(connection, artifact_id, "approve", expected_revision, now)
}

pub(crate) fn activate(
    connection: &Connection,
    artifact_id: &str,
    revision: i64,
    now: i64,
) -> Result<(), String> {
    crate::adaptive_improvement::activate(connection, artifact_id, revision, now)?;
    receipt(connection, artifact_id, "activate", Some(revision), now)
}

fn receipt(
    connection: &Connection,
    artifact_id: &str,
    kind: &str,
    expected_revision: Option<i64>,
    now: i64,
) -> Result<(), String> {
    connection
        .execute(
            "INSERT INTO ai_evaluation_receipts(id,artifact_id,kind,expected_revision,created_at_ms)
             VALUES(?1,?2,?3,?4,?5)",
            params![new_id("ai_receipt"), artifact_id, kind, expected_revision, now],
        )
        .map_err(database_error)?;
    Ok(())
}

fn digest(value: String) -> String {
    format!("{:x}", Sha256::digest(value.as_bytes()))
}

#[derive(Debug, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AdaptiveEvaluateInput {
    pub bundle: EvaluationBundle,
}

#[derive(Debug, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AdaptiveArtifactAction {
    pub artifact_id: String,
    pub expected_revision: Option<i64>,
}

#[tauri::command]
pub(crate) fn list_adaptive_evaluations(
    state: tauri::State<'_, crate::AppState>,
) -> Result<Vec<EvaluationView>, String> {
    state.sqlite_readers.read(list_views)
}

#[tauri::command]
pub(crate) fn import_adaptive_evaluation(
    state: tauri::State<'_, crate::AppState>,
    input: AdaptiveEvaluateInput,
) -> Result<String, String> {
    if input.bundle.artifact_id.is_empty() || input.bundle.pairs.is_empty() {
        return Err("evaluation_bundle_invalid".into());
    }
    state.sqlite_writer.write(|connection| {
        let state = import_bundle(connection, &input.bundle, now_ms())?;
        Ok(state.to_string())
    })
}

#[tauri::command]
pub(crate) fn approve_adaptive_artifact(
    state: tauri::State<'_, crate::AppState>,
    input: AdaptiveArtifactAction,
) -> Result<(), String> {
    state.sqlite_writer.write(|connection| {
        approve(
            connection,
            &input.artifact_id,
            input.expected_revision,
            now_ms(),
        )
    })
}

#[tauri::command]
pub(crate) fn activate_adaptive_artifact(
    state: tauri::State<'_, crate::AppState>,
    input: AdaptiveArtifactAction,
) -> Result<(), String> {
    let revision = input.expected_revision.ok_or("revision_required")?;
    state
        .sqlite_writer
        .write(|connection| activate(connection, &input.artifact_id, revision, now_ms()))
}

pub(crate) fn typescript_bindings() -> String {
    fn declaration<T: TS>() -> String {
        format!("export {}", T::decl(&ts_rs::Config::default()))
    }
    [
        declaration::<EvaluationPair>(),
        declaration::<EvaluationBundle>(),
        declaration::<EvaluationView>(),
        declaration::<AdaptiveEvaluateInput>(),
        declaration::<AdaptiveArtifactAction>(),
    ]
    .join("\n\n")
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adaptive_improvement::{self, Domain};
    use crate::persistence::schema::initialize_database;
    use rusqlite::Connection;

    fn db() -> Connection {
        let connection = Connection::open_in_memory().unwrap();
        initialize_database(&connection).unwrap();
        connection
    }

    #[test]
    fn rf5_a_01_and_03_rejects_incomplete_and_overlapping_groups() {
        let connection = db();
        let artifact = adaptive_improvement::create_artifact(
            &connection,
            Domain::Plan,
            "scope",
            &["rules".into(), "learned".into()],
            r#"{"learned":1.0,"rules":0.0}"#,
            1,
            1,
        )
        .unwrap();
        let err = import_bundle(
            &connection,
            &EvaluationBundle {
                artifact_id: artifact.clone(),
                dataset_digest: "ds".into(),
                seed: 7,
                pairs: vec![EvaluationPair {
                    group_key: "g".into(),
                    split: "eval".into(),
                    candidate_success: 1.0,
                    rules_success: 0.0,
                    candidate_resource: Some(1.0),
                    rules_resource: None,
                    candidate_other_resource: None,
                    rules_other_resource: None,
                    evidence_digest: "e".into(),
                }],
            },
            1,
        )
        .unwrap_err();
        assert_eq!(err, "pair_unobserved");
        assert_eq!(resource_ratio(95.0, 100.0).unwrap(), 0.05);
        assert!(resource_ratio(1.0, 0.0).is_err());
    }

    #[test]
    fn rf5_a_06_active_artifact_changes_choose_and_rollback_returns_rules() {
        let connection = db();
        let candidates = ["rules".to_string(), "learned".to_string()];
        let artifact = adaptive_improvement::create_artifact(
            &connection,
            Domain::Notification,
            "goal",
            &candidates,
            r#"{"learned":2.0,"rules":0.1}"#,
            1,
            1,
        )
        .unwrap();
        connection
            .execute(
                "UPDATE ai_artifacts SET state='eligible' WHERE id=?1",
                [&artifact],
            )
            .unwrap();
        adaptive_improvement::activate(&connection, &artifact, 1, 10).unwrap();
        let (selected, mode, _) = adaptive_improvement::choose(
            &connection,
            Domain::Notification,
            "goal",
            &candidates,
            "rules",
            20,
        )
        .unwrap();
        assert_eq!(selected, "learned");
        assert_eq!(mode, "adaptive");
        adaptive_improvement::rollback_active_to_rules(&connection, &artifact, 30).unwrap();
        let (selected, mode, _) = adaptive_improvement::choose(
            &connection,
            Domain::Notification,
            "goal",
            &candidates,
            "rules",
            40,
        )
        .unwrap();
        assert_eq!(selected, "rules");
        assert_eq!(mode, "rules");
    }

    #[test]
    fn rf5_a_03_train_pairs_are_not_evaluated_and_unknown_split_is_rejected() {
        let connection = db();
        let artifact = adaptive_improvement::create_artifact(
            &connection,
            Domain::Plan,
            "scope",
            &["rules".into(), "learned".into()],
            r#"{"learned":1.0,"rules":0.0}"#,
            1,
            1,
        )
        .unwrap();
        let train_only = import_bundle(
            &connection,
            &EvaluationBundle {
                artifact_id: artifact.clone(),
                dataset_digest: "ds-train".into(),
                seed: 1,
                pairs: vec![EvaluationPair {
                    group_key: "g".into(),
                    split: "train".into(),
                    candidate_success: 1.0,
                    rules_success: 0.0,
                    candidate_resource: None,
                    rules_resource: None,
                    candidate_other_resource: None,
                    rules_other_resource: None,
                    evidence_digest: "e".into(),
                }],
            },
            1,
        )
        .unwrap_err();
        assert_eq!(train_only, "eval_split_empty");
        let unknown = import_bundle(
            &connection,
            &EvaluationBundle {
                artifact_id: artifact,
                dataset_digest: "ds-hold".into(),
                seed: 1,
                pairs: vec![EvaluationPair {
                    group_key: "g".into(),
                    split: "holdout".into(),
                    candidate_success: 1.0,
                    rules_success: 0.0,
                    candidate_resource: None,
                    rules_resource: None,
                    candidate_other_resource: None,
                    rules_other_resource: None,
                    evidence_digest: "e".into(),
                }],
            },
            1,
        )
        .unwrap_err();
        assert_eq!(unknown, "unknown_split");
    }

    #[test]
    fn rf5_a_04_shadow_counts_only_scoped_observed_decisions() {
        let connection = db();
        let artifact = adaptive_improvement::create_artifact(
            &connection,
            Domain::Plan,
            "scope",
            &["rules".into(), "learned".into()],
            r#"{"learned":1.0,"rules":0.0}"#,
            1,
            1,
        )
        .unwrap();
        connection
            .execute(
                "UPDATE ai_artifacts SET state='shadow' WHERE id=?1",
                [&artifact],
            )
            .unwrap();
        let (digest, fingerprint): (String, String) = connection
            .query_row(
                "SELECT digest,candidate_fingerprint FROM ai_artifacts WHERE id=?1",
                [&artifact],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO ai_evaluation_records(
                   id,artifact_id,dataset_digest,artifact_digest,domain,scope_key,candidate_fingerprint,
                   evaluator_version,seed,group_split_json,summary_json,source_kind,created_at_ms)
                 VALUES('rec',?1,'ds',?2,'plan','scope',?3,'rf5-eval-1',1,'{}','{}','host_bundle',10)",
                rusqlite::params![artifact, digest, fingerprint],
            )
            .unwrap();
        assert_eq!(
            approve(&connection, &artifact, None, 20).unwrap_err(),
            "shadow_observations_insufficient"
        );
        for index in 0..30 {
            let id = format!("ai-plan-{index}");
            connection
                .execute(
                    "INSERT INTO ai_decisions(
                       id,domain,scope_key,event_seq,policy_revision,candidate_fingerprint,eligible_json,
                       selected,selection_mode,source_refs_json,created_at_ms)
                     VALUES(?1,'plan','scope',?2,0,?3,'[\"rules\",\"learned\"]','rules','rules','{}',11)",
                    rusqlite::params![id, index, fingerprint],
                )
                .unwrap();
            connection
                .execute(
                    "INSERT INTO ai_outcomes(decision_id,technical_success,verifier_success,user_acceptance,correction,latency_ms,cost_micros,usefulness,source_id,revision,created_at_ms)
                     VALUES(?1,1,1,1,0,1,1,1,'src',1,11)",
                    rusqlite::params![format!("ai-plan-{index}")],
                )
                .unwrap();
        }
        connection
            .execute(
                "INSERT INTO ai_decisions(
                   id,domain,scope_key,event_seq,policy_revision,candidate_fingerprint,eligible_json,
                   selected,selection_mode,source_refs_json,created_at_ms)
                 VALUES('ai-other','notification','other',1,0,'ffff','[\"rules\"]','rules','adaptive','{}',11)",
                [],
            )
            .unwrap();
        approve(&connection, &artifact, None, 21).unwrap();
        let state: String = connection
            .query_row(
                "SELECT state FROM ai_artifacts WHERE id=?1",
                [&artifact],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(state, "eligible");
    }
}
