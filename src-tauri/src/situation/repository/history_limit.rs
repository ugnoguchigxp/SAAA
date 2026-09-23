use super::*;
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
    super::super::validate_settings(&settings)?;
    Ok(settings)
}
pub fn save_enabled(
    connection: &Connection,
    enabled: bool,
) -> Result<SituationRuntimeSettings, String> {
    let mut settings = load_settings(connection)?;
    settings.enabled = enabled;
    super::super::validate_settings(&settings)?;
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
        super::super::validate_scene(scene)?;
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
pub(super) fn ledger_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<SituationLedgerEntry> {
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
pub(super) fn decode_json_column<T: DeserializeOwned>(index: usize, value: &str) -> rusqlite::Result<T> {
    serde_json::from_str(value).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(
            index,
            rusqlite::types::Type::Text,
            Box::new(error),
        )
    })
}
pub(super) fn conversion_error(index: usize, message: &str) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(
        index,
        rusqlite::types::Type::Text,
        Box::new(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            message.to_string(),
        )),
    )
}
