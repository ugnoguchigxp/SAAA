use super::contracts::{
    QualityWindowCounters, ShadowDecision, SituationEvaluationSummary, SituationFeedback,
    SituationFeedbackInput, SituationLedgerEntry, SituationQualityMetrics,
    SituationRuntimeSettings, SituationState,
};
use rusqlite::{params, Connection, OptionalExtension};
use serde::de::DeserializeOwned;

const HISTORY_LIMIT: i64 = 100;

pub fn load_settings(connection: &Connection) -> Result<SituationRuntimeSettings, String> {
    let value: String = connection
        .query_row(
            "SELECT value_json FROM settings_documents WHERE namespace = 'situation.runtime' AND key = 'default'",
            [],
            |row| row.get(0),
        )
        .map_err(crate::database_error)?;
    let settings = serde_json::from_str(&value)
        .map_err(|error| format!("Invalid Situation settings: {error}"))?;
    super::validate_settings(&settings)?;
    Ok(settings)
}

pub fn save_enabled(
    connection: &Connection,
    enabled: bool,
) -> Result<SituationRuntimeSettings, String> {
    let mut settings = load_settings(connection)?;
    settings.enabled = enabled;
    super::validate_settings(&settings)?;
    connection
        .execute(
            "UPDATE settings_documents SET value_json = ?1, updated_at = ?2
             WHERE namespace = 'situation.runtime' AND key = 'default'",
            params![
                serde_json::to_string(&settings).map_err(|error| error.to_string())?,
                crate::now_iso()
            ],
        )
        .map_err(crate::database_error)?;
    Ok(settings)
}

pub fn persist_entry(connection: &Connection, entry: &SituationLedgerEntry) -> Result<(), String> {
    validate_ledger_entry(entry)?;
    let evidence = serde_json::to_string(&entry.state.evidence)
        .map_err(|error| format!("Could not encode Situation evidence: {error}"))?;
    let health = serde_json::to_string(&entry.signal_health)
        .map_err(|error| format!("Could not encode Signal health: {error}"))?;
    let decision_reasons = serde_json::to_string(&entry.decision.reason_codes)
        .map_err(|error| format!("Could not encode Shadow decision reasons: {error}"))?;
    connection
        .execute(
            "INSERT INTO situation_ledger(
               id, observed_at, scene, confidence, user_attention, audio_environment,
               proposed_attention, actual_execution, actual_presentation, evidence_json,
               signal_health_json, decision_reasons_json, rule_version, policy_version, entry_kind
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'NONE', 'SILENT', ?8, ?9, ?10, ?11, ?12, ?13)",
            params![
                entry.id,
                entry.observed_at,
                entry.state.scene,
                entry.state.confidence,
                entry.state.user_attention,
                entry.state.audio_environment,
                entry.decision.proposed_attention,
                evidence,
                health,
                decision_reasons,
                entry.state.rule_version,
                entry.decision.policy_version,
                entry.entry_kind,
            ],
        )
        .map_err(crate::database_error)?;
    Ok(())
}

pub fn persist_entry_with_retention(
    connection: &Connection,
    entry: &SituationLedgerEntry,
    settings: &SituationRuntimeSettings,
    now_ms: u128,
    quality_window: Option<(u128, &str, &QualityWindowCounters)>,
) -> Result<(), String> {
    let transaction = connection
        .unchecked_transaction()
        .map_err(crate::database_error)?;
    persist_entry(&transaction, entry)?;
    if let Some((started_at_ms, rule_version, counters)) = quality_window {
        validate_quality_counters(counters)?;
        let counters_json = serde_json::to_string(counters)
            .map_err(|error| format!("Could not encode Situation quality window: {error}"))?;
        if counters_json.len() > 4_096 {
            return Err("Situation quality window is too large".to_string());
        }
        transaction
            .execute(
                "INSERT INTO situation_quality_windows(id,started_at,ended_at,rule_version,counters_json,created_at)
                 VALUES(?1,?2,?3,?4,?5,?6)",
                params![
                    crate::new_id("situation_quality"),
                    started_at_ms.to_string(),
                    now_ms.to_string(),
                    rule_version,
                    counters_json,
                    crate::now_iso()
                ],
            )
            .map_err(crate::database_error)?;
    }
    apply_retention(&transaction, settings, now_ms)?;
    transaction.commit().map_err(crate::database_error)
}

pub fn persist_quality_window(
    connection: &Connection,
    started_at_ms: u128,
    ended_at_ms: u128,
    rule_version: &str,
    counters: &QualityWindowCounters,
    settings: &SituationRuntimeSettings,
) -> Result<(), String> {
    if counters.sample_count == 0 {
        return Ok(());
    }
    validate_quality_counters(counters)?;
    let counters_json = serde_json::to_string(counters)
        .map_err(|error| format!("Could not encode Situation quality window: {error}"))?;
    if counters_json.len() > 4_096 {
        return Err("Situation quality window is too large".to_string());
    }
    let transaction = connection
        .unchecked_transaction()
        .map_err(crate::database_error)?;
    transaction
        .execute(
            "INSERT INTO situation_quality_windows(id,started_at,ended_at,rule_version,counters_json,created_at)
             VALUES(?1,?2,?3,?4,?5,?6)",
            params![
                crate::new_id("situation_quality"),
                started_at_ms.to_string(),
                ended_at_ms.to_string(),
                rule_version,
                counters_json,
                crate::now_iso()
            ],
        )
        .map_err(crate::database_error)?;
    apply_retention(&transaction, settings, ended_at_ms)?;
    transaction.commit().map_err(crate::database_error)
}

pub fn apply_retention(
    connection: &Connection,
    settings: &SituationRuntimeSettings,
    now_ms: u128,
) -> Result<(), String> {
    let cutoff = now_ms.saturating_sub(u128::from(settings.retention_days) * 86_400_000);
    let cutoff = i64::try_from(cutoff).unwrap_or(i64::MAX);
    connection
        .execute(
            "DELETE FROM situation_ledger WHERE CAST(observed_at AS INTEGER) < ?1",
            params![cutoff],
        )
        .map_err(crate::database_error)?;
    connection
        .execute(
            "DELETE FROM situation_ledger WHERE id IN (
               SELECT id FROM situation_ledger ORDER BY CAST(observed_at AS INTEGER) DESC
               LIMIT -1 OFFSET ?1
             )",
            params![settings.max_ledger_entries],
        )
        .map_err(crate::database_error)?;
    connection
        .execute(
            "DELETE FROM situation_quality_windows WHERE CAST(ended_at AS INTEGER) < ?1",
            params![cutoff],
        )
        .map_err(crate::database_error)?;
    connection
        .execute(
            "DELETE FROM situation_quality_windows WHERE id IN (
               SELECT id FROM situation_quality_windows ORDER BY CAST(ended_at AS INTEGER) DESC
               LIMIT -1 OFFSET 1000
             )",
            [],
        )
        .map_err(crate::database_error)?;
    Ok(())
}

pub fn quality_metrics(connection: &Connection) -> Result<SituationQualityMetrics, String> {
    let mut statement = connection
        .prepare(
            "SELECT counters_json FROM situation_quality_windows
             ORDER BY CAST(ended_at AS INTEGER) DESC LIMIT 1000",
        )
        .map_err(crate::database_error)?;
    let encoded = statement
        .query_map([], |row| row.get::<_, String>(0))
        .map_err(crate::database_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(crate::database_error)?;
    let mut total = QualityWindowCounters::default();
    for value in encoded {
        let counters: QualityWindowCounters = serde_json::from_str(&value)
            .map_err(|error| format!("Invalid Situation quality window: {error}"))?;
        validate_quality_counters(&counters)?;
        total.sample_count = total.sample_count.saturating_add(counters.sample_count);
        total.candidate_change_count = total
            .candidate_change_count
            .saturating_add(counters.candidate_change_count);
        total.stale_owned_signal_count = total
            .stale_owned_signal_count
            .saturating_add(counters.stale_owned_signal_count);
    }
    let enough_data = total.sample_count >= 20;
    Ok(SituationQualityMetrics {
        sample_count: total.sample_count,
        flapping_rate: enough_data
            .then(|| total.candidate_change_count as f64 / total.sample_count as f64),
        stale_rate: enough_data
            .then(|| total.stale_owned_signal_count as f64 / total.sample_count as f64),
    })
}

pub fn list_history(connection: &Connection) -> Result<Vec<SituationLedgerEntry>, String> {
    let mut statement = connection
        .prepare(
            "SELECT l.id, l.observed_at, l.scene, l.confidence, l.user_attention,
                    l.audio_environment, l.proposed_attention, l.actual_execution,
                    l.actual_presentation, l.evidence_json, l.signal_health_json,
                    l.decision_reasons_json, l.rule_version, l.policy_version, l.entry_kind,
                    f.verdict, f.impact, f.corrected_scene, f.reason_code, f.created_at
             FROM situation_ledger l
             LEFT JOIN situation_feedback f ON f.ledger_id = l.id
             ORDER BY CAST(l.observed_at AS INTEGER) DESC LIMIT ?1",
        )
        .map_err(crate::database_error)?;
    let entries = statement
        .query_map(params![HISTORY_LIMIT], ledger_from_row)
        .map_err(crate::database_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(crate::database_error)?;
    Ok(entries)
}

pub fn latest_entry(connection: &Connection) -> Result<Option<SituationLedgerEntry>, String> {
    connection
        .query_row(
            "SELECT l.id, l.observed_at, l.scene, l.confidence, l.user_attention,
                    l.audio_environment, l.proposed_attention, l.actual_execution,
                    l.actual_presentation, l.evidence_json, l.signal_health_json,
                    l.decision_reasons_json, l.rule_version, l.policy_version, l.entry_kind,
                    f.verdict, f.impact, f.corrected_scene, f.reason_code, f.created_at
             FROM situation_ledger l
             LEFT JOIN situation_feedback f ON f.ledger_id = l.id
             ORDER BY CAST(l.observed_at AS INTEGER) DESC LIMIT 1",
            [],
            ledger_from_row,
        )
        .optional()
        .map_err(crate::database_error)
}

pub fn feedback_queue(connection: &Connection) -> Result<Vec<SituationLedgerEntry>, String> {
    let mut statement = connection
        .prepare(
            "SELECT l.id, l.observed_at, l.scene, l.confidence, l.user_attention,
                    l.audio_environment, l.proposed_attention, l.actual_execution,
                    l.actual_presentation, l.evidence_json, l.signal_health_json,
                    l.decision_reasons_json, l.rule_version, l.policy_version, l.entry_kind,
                    NULL, NULL, NULL, NULL, NULL
             FROM situation_ledger l
             LEFT JOIN situation_feedback f ON f.ledger_id = l.id
             WHERE f.ledger_id IS NULL
             ORDER BY CAST(l.observed_at AS INTEGER) DESC LIMIT 50",
        )
        .map_err(crate::database_error)?;
    let entries = statement
        .query_map([], ledger_from_row)
        .map_err(crate::database_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(crate::database_error)?;
    Ok(entries)
}

pub fn evaluation_summary(connection: &Connection) -> Result<SituationEvaluationSummary, String> {
    connection
        .query_row(
            "SELECT
               (SELECT COUNT(*) FROM situation_ledger),
               COALESCE(SUM(CASE WHEN verdict = 'accurate' THEN 1 ELSE 0 END), 0),
               COALESCE(SUM(CASE WHEN verdict = 'inaccurate' THEN 1 ELSE 0 END), 0),
               COALESCE(SUM(CASE WHEN verdict = 'unsure' THEN 1 ELSE 0 END), 0)
             FROM situation_feedback",
            [],
            |row| {
                Ok(SituationEvaluationSummary {
                    total_entries: row.get(0)?,
                    accurate: row.get(1)?,
                    inaccurate: row.get(2)?,
                    unsure: row.get(3)?,
                })
            },
        )
        .map_err(crate::database_error)
}

pub fn submit_feedback(
    connection: &Connection,
    input: &SituationFeedbackInput,
) -> Result<(), String> {
    crate::validate_identifier(&input.ledger_id, "Situation ledger id")?;
    if !matches!(input.verdict.as_str(), "accurate" | "inaccurate" | "unsure") {
        return Err("Invalid Situation feedback verdict".to_string());
    }
    if !matches!(input.impact.as_str(), "none" | "no-effect" | "harmful") {
        return Err("Invalid Situation feedback impact".to_string());
    }
    let valid_reason = matches!(
        input.reason_code.as_deref(),
        Some(
            "wrong-scene"
                | "stale-signal"
                | "unstable-transition"
                | "unwanted-suggestion"
                | "missed-meeting-candidate"
                | "insufficient-evidence"
        )
    );
    if input.reason_code.is_some() && !valid_reason {
        return Err("Invalid Situation feedback reason code".to_string());
    }
    if (input.verdict == "inaccurate" || input.impact == "harmful") && !valid_reason {
        return Err("A reason code is required for inaccurate or harmful feedback".to_string());
    }
    if let Some(scene) = &input.corrected_scene {
        super::validate_scene(scene)?;
    }
    let proposed_attention: Option<String> = connection
        .query_row(
            "SELECT proposed_attention FROM situation_ledger WHERE id = ?1",
            params![input.ledger_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(crate::database_error)?;
    let proposed_attention =
        proposed_attention.ok_or_else(|| "Situation ledger entry does not exist".to_string())?;
    if input.impact == "no-effect" && proposed_attention != "SUGGEST" {
        return Err("No-effect feedback is only valid for a suggested action".to_string());
    }
    connection
        .execute(
            "INSERT INTO situation_feedback(ledger_id, verdict, impact, corrected_scene, reason_code, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(ledger_id) DO UPDATE SET
               verdict = excluded.verdict,
               impact = excluded.impact,
               corrected_scene = excluded.corrected_scene,
               reason_code = excluded.reason_code,
               created_at = excluded.created_at",
            params![
                input.ledger_id,
                input.verdict,
                input.impact,
                input.corrected_scene,
                input.reason_code,
                crate::now_iso()
            ],
        )
        .map_err(crate::database_error)?;
    Ok(())
}

pub fn clear_history(connection: &Connection) -> Result<(), String> {
    let transaction = connection
        .unchecked_transaction()
        .map_err(crate::database_error)?;
    transaction
        .execute("DELETE FROM situation_ledger", [])
        .map_err(crate::database_error)?;
    transaction
        .execute("DELETE FROM situation_quality_windows", [])
        .map_err(crate::database_error)?;
    transaction
        .execute("DELETE FROM situation_calibration_runs", [])
        .map_err(crate::database_error)?;
    transaction.commit().map_err(crate::database_error)?;
    Ok(())
}

fn ledger_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<SituationLedgerEntry> {
    let observed_at: String = row.get(1)?;
    let evidence_text: String = row.get(9)?;
    let health_text: String = row.get(10)?;
    let reason_text: String = row.get(11)?;
    let feedback = match row.get::<_, Option<String>>(15)? {
        Some(verdict) => Some(SituationFeedback {
            verdict,
            impact: row.get(16)?,
            corrected_scene: row.get(17)?,
            reason_code: row.get(18)?,
            created_at: row
                .get::<_, Option<String>>(19)?
                .ok_or_else(|| conversion_error(19, "Feedback timestamp is missing"))?,
        }),
        None => None,
    };
    let entry = SituationLedgerEntry {
        id: row.get(0)?,
        observed_at: observed_at.clone(),
        state: SituationState {
            scene: row.get(2)?,
            confidence: row.get(3)?,
            user_attention: row.get(4)?,
            audio_environment: row.get(5)?,
            evidence: decode_json_column(9, &evidence_text)?,
            candidate_since: observed_at.clone(),
            stable_since: observed_at.clone(),
            updated_at: observed_at.clone(),
            rule_version: row.get(12)?,
        },
        decision: ShadowDecision {
            mode: "shadow".to_string(),
            proposed_attention: row.get(6)?,
            actual_execution: row.get(7)?,
            actual_presentation: row.get(8)?,
            reason_codes: decode_json_column(11, &reason_text)?,
            decided_at: observed_at.clone(),
            policy_version: row.get(13)?,
        },
        signal_health: decode_json_column(10, &health_text)?,
        entry_kind: row.get(14)?,
        feedback,
    };
    validate_ledger_entry(&entry).map_err(|error| conversion_error(0, &error))?;
    Ok(entry)
}

fn decode_json_column<T: DeserializeOwned>(index: usize, value: &str) -> rusqlite::Result<T> {
    serde_json::from_str(value).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(
            index,
            rusqlite::types::Type::Text,
            Box::new(error),
        )
    })
}

fn conversion_error(index: usize, message: &str) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(
        index,
        rusqlite::types::Type::Text,
        Box::new(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            message.to_string(),
        )),
    )
}

fn validate_ledger_entry(entry: &SituationLedgerEntry) -> Result<(), String> {
    crate::validate_identifier(&entry.id, "Situation ledger id")?;
    super::validate_scene(&entry.state.scene)?;
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
            super::validate_scene(scene)?;
        }
    }
    Ok(())
}

fn bounded_code(code: &str) -> bool {
    !code.is_empty()
        && code
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || character == '-')
}

fn bounded_version(version: &str) -> bool {
    !version.is_empty()
        && version.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.')
        })
}

fn validate_quality_counters(counters: &QualityWindowCounters) -> Result<(), String> {
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
