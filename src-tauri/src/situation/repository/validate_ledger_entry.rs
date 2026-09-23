use super::*;
pub(super) fn validate_ledger_entry(entry: &SituationLedgerEntry) -> Result<(), String> {
    crate::validate_identifier(&entry.id, "Situation ledger id")?;
    super::super::validate_scene(&entry.state.scene)?;
    if entry.observed_at.parse::<u128>().is_err()
        || entry.state.confidence > 100
        || entry.state.evidence.len() > 16
        || entry.signal_health.len() > 8
        || entry.decision.reason_codes.len() > 8
        || entry.decision.mode != "shadow"
        || !matches!(
            entry.decision.proposed_attention.as_str(),
            "IGNORE" | "OBSERVE" | "SUGGEST" | "RESPOND"
        )
        || !matches!(
            entry.entry_kind.as_str(),
            "transition" | "decision" | "heartbeat"
        )
        || entry.decision.actual_execution != "NONE"
        || entry.decision.actual_presentation != "SILENT"
    {
        return Err("Invalid or unbounded Situation ledger entry".to_string());
    }
    if entry.state.evidence.iter().any(|item| {
        item.code.len() > 80 || !bounded_code(&item.code) || !(-100..=100).contains(&item.weight)
    }) || entry
        .decision
        .reason_codes
        .iter()
        .any(|code| code.len() > 80 || !bounded_code(code))
    {
        return Err("Situation evidence must use bounded reason codes".to_string());
    }
    if !matches!(
        entry.state.user_attention.as_str(),
        "available" | "busy" | "unknown"
    ) || !matches!(
        entry.state.audio_environment.as_str(),
        "silence" | "speech" | "multi-speaker" | "media" | "unknown"
    ) || entry.state.rule_version.len() > 160
        || !bounded_version(&entry.state.rule_version)
        || entry.decision.policy_version.len() > 160
        || !bounded_version(&entry.decision.policy_version)
        || entry
            .signal_health
            .iter()
            .any(|item| item.source.len() > 80 || !bounded_code(&item.source))
    {
        return Err("Invalid Situation state metadata".to_string());
    }
    if let Some(feedback) = &entry.feedback {
        if !matches!(
            feedback.verdict.as_str(),
            "accurate" | "inaccurate" | "unsure"
        ) {
            return Err("Invalid Situation feedback verdict".to_string());
        }
        if !matches!(feedback.impact.as_str(), "none" | "no-effect" | "harmful") {
            return Err("Invalid Situation feedback impact".to_string());
        }
        let valid_reason = matches!(
            feedback.reason_code.as_deref(),
            None | Some(
                "wrong-scene"
                    | "stale-signal"
                    | "unstable-transition"
                    | "unwanted-suggestion"
                    | "missed-meeting-candidate"
                    | "insufficient-evidence"
            )
        );
        if !valid_reason
            || ((feedback.verdict == "inaccurate" || feedback.impact == "harmful")
                && feedback.reason_code.is_none())
            || (feedback.impact == "no-effect" && entry.decision.proposed_attention != "SUGGEST")
            || feedback.created_at.parse::<u128>().is_err()
        {
            return Err("Invalid Situation feedback combination".to_string());
        }
        if let Some(scene) = &feedback.corrected_scene {
            super::super::validate_scene(scene)?;
        }
    }
    Ok(())
}
pub(super) fn bounded_code(code: &str) -> bool {
    !code.is_empty()
        && code
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || character == '-')
}
pub(super) fn bounded_version(version: &str) -> bool {
    !version.is_empty()
        && version.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.')
        })
}
pub(super) fn validate_quality_counters(counters: &QualityWindowCounters) -> Result<(), String> {
    let sample_count = counters.sample_count;
    let per_sample = [
        counters.candidate_change_count,
        counters.stable_transition_count,
        counters.unknown_sample_count,
        counters.stale_owned_signal_count,
        counters.decision_ignore_count,
        counters.decision_observe_count,
        counters.decision_suggest_count,
        counters.decision_respond_count,
    ];
    let health = [
        counters.health_ready_count,
        counters.health_disabled_count,
        counters.health_permission_denied_count,
        counters.health_unsupported_count,
        counters.health_degraded_count,
    ];
    let health_total = health.into_iter().fold(0_u64, u64::saturating_add);
    let health_limit = sample_count.saturating_mul(5);
    if per_sample.iter().any(|count| *count > sample_count) || health_total > health_limit {
        return Err("Invalid Situation quality counters".to_string());
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::situation::contracts::{
        initial_decision, initial_state, Evidence, SignalHealth, SignalHealthEntry,
    };

    fn entry(index: u64, observed_at: &str) -> SituationLedgerEntry {
        let mut state = initial_state(observed_at);
        state.scene = if index.is_multiple_of(2) {
            "CODING"
        } else {
            "SOLO"
        }
        .to_string();
        state.confidence = 80;
        SituationLedgerEntry {
            id: format!("situation_{index}"),
            observed_at: observed_at.to_string(),
            state,
            decision: initial_decision(observed_at),
            signal_health: vec![SignalHealthEntry {
                source: "foreground".to_string(),
                health: SignalHealth::Ready,
            }],
            entry_kind: "heartbeat".to_string(),
            feedback: None,
        }
    }

    #[test]
    fn ledger_rejects_unbounded_or_raw_evidence() {
        let now = crate::now_iso();
        let mut entry = SituationLedgerEntry {
            id: crate::new_id("situation"),
            observed_at: now.clone(),
            state: initial_state(&now),
            decision: initial_decision(&now),
            signal_health: vec![SignalHealthEntry {
                source: "foreground".to_string(),
                health: SignalHealth::Ready,
            }],
            entry_kind: "heartbeat".to_string(),
            feedback: None,
        };
        entry.state.evidence.push(Evidence {
            code: "raw window title".to_string(),
            weight: 50,
        });
        assert!(validate_ledger_entry(&entry).is_err());
    }

    #[test]
    fn retention_is_bounded_and_feedback_cascades_with_history() {
        let connection = Connection::open_in_memory().expect("database opens");
        crate::initialize_database(&connection).expect("database initializes");
        for index in 0..150 {
            persist_entry(&connection, &entry(index, &(10_000 + index).to_string()))
                .expect("entry persists");
        }
        submit_feedback(
            &connection,
            &SituationFeedbackInput {
                ledger_id: "situation_149".to_string(),
                verdict: "accurate".to_string(),
                impact: "none".to_string(),
                corrected_scene: None,
                reason_code: None,
            },
        )
        .expect("feedback persists");
        let settings = SituationRuntimeSettings {
            max_ledger_entries: 100,
            retention_days: 30,
            ..SituationRuntimeSettings::default()
        };
        apply_retention(&connection, &settings, 20_000).expect("retention applies");
        let count: i64 = connection
            .query_row("SELECT COUNT(*) FROM situation_ledger", [], |row| {
                row.get(0)
            })
            .expect("ledger count");
        assert_eq!(count, 100);
        let summary = evaluation_summary(&connection).expect("summary loads");
        assert_eq!(summary.accurate, 1);
        clear_history(&connection).expect("history clears");
        let feedback_count: i64 = connection
            .query_row("SELECT COUNT(*) FROM situation_feedback", [], |row| {
                row.get(0)
            })
            .expect("feedback count");
        assert_eq!(feedback_count, 0);
    }

    #[test]
    fn feedback_rejects_unknown_reasons_and_no_effect_without_a_suggestion() {
        let connection = Connection::open_in_memory().expect("database opens");
        crate::initialize_database(&connection).expect("database initializes");
        persist_entry(&connection, &entry(1, "1000")).expect("entry persists");

        let invalid_reason = SituationFeedbackInput {
            ledger_id: "situation_1".to_string(),
            verdict: "accurate".to_string(),
            impact: "none".to_string(),
            corrected_scene: None,
            reason_code: Some("free-form-reason".to_string()),
        };
        assert!(submit_feedback(&connection, &invalid_reason).is_err());

        let invalid_impact = SituationFeedbackInput {
            ledger_id: "situation_1".to_string(),
            verdict: "accurate".to_string(),
            impact: "no-effect".to_string(),
            corrected_scene: None,
            reason_code: None,
        };
        assert!(submit_feedback(&connection, &invalid_impact).is_err());
    }

    #[test]
    fn quality_metrics_require_twenty_samples_and_decode_strictly() {
        let connection = Connection::open_in_memory().expect("database opens");
        crate::initialize_database(&connection).expect("database initializes");
        let settings = SituationRuntimeSettings::default();
        persist_quality_window(
            &connection,
            1,
            2,
            "mvp1-rules-v1",
            &QualityWindowCounters {
                sample_count: 20,
                candidate_change_count: 2,
                stale_owned_signal_count: 4,
                ..QualityWindowCounters::default()
            },
            &settings,
        )
        .expect("quality persists");
        let metrics = quality_metrics(&connection).expect("quality loads");
        assert_eq!(metrics.sample_count, 20);
        assert_eq!(metrics.flapping_rate, Some(0.1));
        assert_eq!(metrics.stale_rate, Some(0.2));
        connection
            .execute(
                "UPDATE situation_quality_windows
                 SET counters_json = json_set(counters_json, '$.candidateChangeCount', 21)",
                [],
            )
            .expect("fixture corrupts counters");
        assert!(quality_metrics(&connection).is_err());
    }

    #[test]
    fn quality_counters_allow_five_health_sources_per_sample() {
        let mut counters = QualityWindowCounters {
            sample_count: 20,
            health_ready_count: 100,
            ..QualityWindowCounters::default()
        };
        validate_quality_counters(&counters).expect("five health sources are valid");
        counters.health_ready_count = 101;
        assert!(validate_quality_counters(&counters).is_err());
    }

    #[test]
    fn retention_removes_entries_older_than_the_configured_days() {
        let connection = Connection::open_in_memory().expect("database opens");
        crate::initialize_database(&connection).expect("database initializes");
        persist_entry(&connection, &entry(1, "1")).expect("old entry persists");
        persist_entry(&connection, &entry(2, "172800000")).expect("recent entry persists");
        let settings = SituationRuntimeSettings {
            retention_days: 1,
            ..SituationRuntimeSettings::default()
        };
        apply_retention(&connection, &settings, 172_800_000).expect("retention applies");
        let history = list_history(&connection).expect("history loads");
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].id, "situation_2");
    }

    #[test]
    fn corrupted_ledger_json_is_reported_instead_of_silently_discarded() {
        let connection = Connection::open_in_memory().expect("database opens");
        crate::initialize_database(&connection).expect("database initializes");
        persist_entry(&connection, &entry(1, "1000")).expect("entry persists");
        connection
            .execute(
                "UPDATE situation_ledger SET evidence_json = 'not-json' WHERE id = 'situation_1'",
                [],
            )
            .expect("fixture corrupts ledger");
        assert!(list_history(&connection).is_err());
    }

    #[test]
    fn invalid_persisted_runtime_settings_are_rejected_on_load() {
        let connection = Connection::open_in_memory().expect("database opens");
        crate::initialize_database(&connection).expect("database initializes");
        connection
            .execute(
                "UPDATE settings_documents
                 SET value_json = json_set(value_json, '$.sampleIntervalMs', 0)
                 WHERE namespace = 'situation.runtime' AND key = 'default'",
                [],
            )
            .expect("fixture invalidates settings");
        assert!(load_settings(&connection).is_err());
    }

    #[test]
    fn monitoring_enablement_persists_across_database_reopen() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("situation-settings.sqlite3");
        let connection = Connection::open(&path).expect("database opens");
        crate::initialize_database(&connection).expect("database initializes");
        let saved = save_enabled(&connection, true).expect("monitoring enables");
        assert!(saved.enabled);
        drop(connection);

        let reopened = Connection::open(path).expect("database reopens");
        crate::initialize_database(&reopened).expect("database reinitializes");
        assert!(load_settings(&reopened).expect("settings reload").enabled);
    }
}
