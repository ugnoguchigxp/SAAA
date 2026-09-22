#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replay_is_deterministic_and_compares_the_default_baseline() {
        let profile = CalibrationProfile {
            id: "profile_candidate".to_string(),
            rule_version: "mvp15-test".to_string(),
            base_rule_version: Some("mvp1-rules-v1".to_string()),
            status: "candidate".to_string(),
            parameters: CalibrationParameters::default(),
            created_at: "1".to_string(),
            decided_at: None,
            decision_reason_code: None,
        };
        let first = replay_metrics(&profile).expect("first replay");
        let second = replay_metrics(&profile).expect("second replay");
        assert_eq!(first, second);
        let metrics: serde_json::Value = serde_json::from_str(&first).expect("metrics decode");
        assert_eq!(metrics["sampleCount"], 17);
        assert_eq!(
            metrics["expectedSceneMatches"],
            metrics["baselineExpectedSceneMatches"]
        );
        assert_eq!(metrics["expectedAttentionSamples"], 14);
        assert_eq!(metrics["expectedAttentionMatches"], 14);
        assert_eq!(metrics["baselineExpectedAttentionMatches"], 14);
        assert_eq!(metrics["deterministic"], true);
    }

    #[test]
    fn v1_fixture_defaults_input_activity_without_changing_the_scene_baseline() {
        let samples: Vec<ReplaySampleV1> =
            serde_json::from_str(include_str!("../../../fixtures/situation/mvp1-v1.json"))
                .expect("v1 fixture remains compatible");
        assert!(samples.iter().all(|sample| {
            sample.signals.input_activity.state
                == super::super::contracts::InputActivityState::Unknown
                && sample.signals.input_activity.health
                    == super::super::contracts::SignalHealth::Unsupported
        }));
        let mut summary = ReplaySummary {
            sample_count: 0,
            expected_scene_matches: 0,
            expected_attention_samples: 0,
            expected_attention_matches: 0,
            policy_counts: [0; 4],
        };
        replay_scenario(
            samples.iter().map(|sample| {
                (
                    sample.elapsed_ms,
                    &sample.signals,
                    sample.expected_scene,
                    None,
                )
            }),
            &CalibrationParameters::default(),
            &mut summary,
        )
        .expect("v1 baseline replays");
        assert_eq!(summary.expected_scene_matches, 1);
    }

    #[test]
    fn accept_and_rollback_require_the_exact_profile_lifecycle() {
        let mut connection = Connection::open_in_memory().expect("database opens");
        crate::initialize_database(&connection).expect("database initializes");
        let candidate = create_candidate(&connection, CalibrationParameters::default())
            .expect("candidate creates");
        let metrics = replay_metrics(&candidate).expect("candidate replays");
        save_run(&connection, &candidate.id, "completed", Some(metrics), None).expect("run saves");

        let active = decide(
            &mut connection,
            &candidate.id,
            "accept",
            "insufficient-evidence",
        )
        .expect("candidate accepts");
        assert_eq!(active.id, candidate.id);
        assert!(decide(
            &mut connection,
            "profile_other",
            "rollback",
            "insufficient-evidence"
        )
        .is_err());
        assert_eq!(
            active_profile(&connection).expect("active remains").id,
            candidate.id
        );
        let restored = decide(
            &mut connection,
            "profile_mvp1_default",
            "rollback",
            "insufficient-evidence",
        )
        .expect("rollback succeeds");
        assert_eq!(restored.id, "profile_mvp1_default");
    }

    #[test]
    fn reject_does_not_succeed_for_a_non_candidate() {
        let mut connection = Connection::open_in_memory().expect("database opens");
        crate::initialize_database(&connection).expect("database initializes");
        assert!(decide(
            &mut connection,
            "profile_mvp1_default",
            "reject",
            "insufficient-evidence"
        )
        .is_err());
        assert_eq!(
            active_profile(&connection)
                .expect("default stays active")
                .id,
            "profile_mvp1_default"
        );
    }

    #[test]
    fn calibration_run_history_is_bounded() {
        let connection = Connection::open_in_memory().expect("database opens");
        crate::initialize_database(&connection).expect("database initializes");
        let candidate = create_candidate(&connection, CalibrationParameters::default())
            .expect("candidate creates");
        let metrics = replay_metrics(&candidate).expect("candidate replays");
        for _ in 0..105 {
            save_run(
                &connection,
                &candidate.id,
                "completed",
                Some(metrics.clone()),
                None,
            )
            .expect("run saves");
        }
        let count: u64 = connection
            .query_row(
                "SELECT COUNT(*) FROM situation_calibration_runs",
                [],
                |row| row.get(0),
            )
            .expect("run count loads");
        assert_eq!(count, 100);
    }
}
