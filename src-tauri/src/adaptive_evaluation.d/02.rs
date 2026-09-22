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
