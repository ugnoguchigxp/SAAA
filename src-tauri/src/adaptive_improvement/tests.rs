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
