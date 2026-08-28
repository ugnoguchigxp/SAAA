use super::contracts::{validate_calibration_parameters, CalibrationParameters, SituationScene};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::json;

pub const FIXTURE_SET_VERSION: &str = "situation-fixtures-v2";

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ReplaySampleV1 {
    elapsed_ms: u64,
    signals: super::contracts::SignalSnapshot,
    expected_scene: SituationScene,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ReplayFixtureSet {
    version: String,
    scenarios: Vec<ReplayScenario>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ReplayScenario {
    id: String,
    samples: Vec<ReplaySampleV2>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ReplaySampleV2 {
    elapsed_ms: u64,
    signals: super::contracts::SignalSnapshot,
    expected_scene: SituationScene,
    expected_attention: String,
}

pub fn replay_metrics(profile: &CalibrationProfile) -> Result<String, String> {
    let v1 = load_v1_samples()?;
    let v2 = load_v2_fixture()?;
    let candidate = replay_all(&v1, &v2, &profile.parameters)?;
    let repeated = replay_all(&v1, &v2, &profile.parameters)?;
    let baseline = replay_all(&v1, &v2, &CalibrationParameters::default())?;
    Ok(json!({
        "fixtureSetVersion": FIXTURE_SET_VERSION,
        "profileRuleVersion": profile.rule_version,
        "sampleCount": candidate.sample_count,
        "expectedSceneMatches": candidate.expected_scene_matches,
        "baselineExpectedSceneMatches": baseline.expected_scene_matches,
        "expectedAttentionSamples": candidate.expected_attention_samples,
        "expectedAttentionMatches": candidate.expected_attention_matches,
        "baselineExpectedAttentionMatches": baseline.expected_attention_matches,
        "shadowPolicyCounts": {
            "ignore": candidate.policy_counts[0],
            "observe": candidate.policy_counts[1],
            "suggest": candidate.policy_counts[2],
            "respond": candidate.policy_counts[3]
        },
        "deterministic": candidate == repeated
    })
    .to_string())
}

fn load_v1_samples() -> Result<Vec<ReplaySampleV1>, String> {
    serde_json::from_str(include_str!("../../fixtures/situation/mvp1-v1.json"))
        .map_err(|_| "Invalid MVP 1 replay fixture".to_string())
}

fn load_v2_fixture() -> Result<ReplayFixtureSet, String> {
    let value: serde_json::Value =
        serde_json::from_str(include_str!("../../fixtures/situation/mvp2.5-v2.json"))
            .map_err(|_| "Invalid situation replay fixture".to_string())?;
    let samples = value
        .get("scenarios")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| "Invalid situation replay fixture".to_string())?
        .iter()
        .filter_map(|scenario| {
            scenario
                .get("samples")
                .and_then(serde_json::Value::as_array)
        })
        .flatten()
        .collect::<Vec<_>>();
    if samples
        .iter()
        .any(|sample| sample.pointer("/signals/inputActivity").is_none())
    {
        return Err("MVP 2.5 replay fixture requires Input Activity".to_string());
    }
    let sample_count = samples.len();
    drop(samples);
    let fixture: ReplayFixtureSet = serde_json::from_value(value)
        .map_err(|_| "Invalid situation replay fixture".to_string())?;
    if fixture.version != FIXTURE_SET_VERSION
        || fixture.scenarios.is_empty()
        || fixture.scenarios.len() > 20
        || sample_count > 14_400
    {
        return Err("Invalid situation replay fixture shape".to_string());
    }
    for scenario in &fixture.scenarios {
        crate::validate_identifier(&scenario.id, "replay scenario id")?;
        if scenario.samples.is_empty() || scenario.samples.len() > 14_400 {
            return Err("Invalid situation replay scenario size".to_string());
        }
        let mut previous_elapsed = None;
        for sample in &scenario.samples {
            if sample.elapsed_ms > 86_400_000
                || previous_elapsed.is_some_and(|previous| sample.elapsed_ms < previous)
                || !matches!(
                    sample.expected_attention.as_str(),
                    "IGNORE" | "OBSERVE" | "SUGGEST" | "RESPOND"
                )
            {
                return Err("Invalid situation replay sample".to_string());
            }
            previous_elapsed = Some(sample.elapsed_ms);
        }
    }
    Ok(fixture)
}

#[derive(Debug, PartialEq, Eq)]
struct ReplaySummary {
    sample_count: u64,
    expected_scene_matches: u64,
    expected_attention_samples: u64,
    expected_attention_matches: u64,
    policy_counts: [u64; 4],
}

fn replay_all(
    v1: &[ReplaySampleV1],
    v2: &ReplayFixtureSet,
    parameters: &CalibrationParameters,
) -> Result<ReplaySummary, String> {
    let mut summary = ReplaySummary {
        sample_count: 0,
        expected_scene_matches: 0,
        expected_attention_samples: 0,
        expected_attention_matches: 0,
        policy_counts: [0; 4],
    };
    let v1_samples = v1.iter().map(|sample| {
        (
            sample.elapsed_ms,
            &sample.signals,
            sample.expected_scene,
            None,
        )
    });
    replay_scenario(v1_samples, parameters, &mut summary)?;
    for scenario in &v2.scenarios {
        let samples = scenario.samples.iter().map(|sample| {
            (
                sample.elapsed_ms,
                &sample.signals,
                sample.expected_scene,
                Some(sample.expected_attention.as_str()),
            )
        });
        replay_scenario(samples, parameters, &mut summary)?;
    }
    Ok(summary)
}

fn replay_scenario<'a>(
    samples: impl Iterator<
        Item = (
            u64,
            &'a super::contracts::SignalSnapshot,
            SituationScene,
            Option<&'a str>,
        ),
    >,
    parameters: &CalibrationParameters,
    summary: &mut ReplaySummary,
) -> Result<(), String> {
    let mut previous_elapsed = None;
    let mut hysteresis =
        super::classifier::Hysteresis::from_state(super::contracts::initial_state("0"));
    for (elapsed_ms, signals, expected_scene, expected_attention) in samples {
        if elapsed_ms > 86_400_000 || previous_elapsed.is_some_and(|previous| elapsed_ms < previous)
        {
            return Err("Invalid situation replay elapsed time".to_string());
        }
        previous_elapsed = Some(elapsed_ms);
        let candidate = super::classifier::classify_with_parameters(signals, parameters);
        let (state, _) = hysteresis.update_with_parameters(
            &candidate,
            &signals.observed_at,
            u128::from(elapsed_ms),
            parameters,
        );
        let attention =
            super::classifier::shadow_policy(&state, signals, &signals.observed_at, parameters)
                .proposed_attention;
        match attention.as_str() {
            "IGNORE" => summary.policy_counts[0] += 1,
            "OBSERVE" => summary.policy_counts[1] += 1,
            "SUGGEST" => summary.policy_counts[2] += 1,
            "RESPOND" => summary.policy_counts[3] += 1,
            _ => return Err("Invalid shadow policy output".to_string()),
        }
        summary.sample_count += 1;
        if state.scene == expected_scene.as_str() {
            summary.expected_scene_matches += 1;
        }
        if let Some(expected_attention) = expected_attention {
            summary.expected_attention_samples += 1;
            if attention == expected_attention {
                summary.expected_attention_matches += 1;
            }
        }
    }
    Ok(())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CalibrationProfile {
    pub id: String,
    pub rule_version: String,
    pub base_rule_version: Option<String>,
    pub status: String,
    pub parameters: CalibrationParameters,
    pub created_at: String,
    pub decided_at: Option<String>,
    pub decision_reason_code: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CalibrationRun {
    pub id: String,
    pub profile_id: String,
    pub fixture_set_version: String,
    pub status: String,
    pub metrics_json: Option<String>,
    pub error_code: Option<String>,
    pub started_at: String,
    pub completed_at: String,
}

pub fn active_profile(connection: &Connection) -> Result<CalibrationProfile, String> {
    profile_by_status(connection, "active")?
        .ok_or_else(|| "Situation calibration has no active profile".to_string())
}
pub fn profile_by_id(connection: &Connection, id: &str) -> Result<CalibrationProfile, String> {
    crate::validate_identifier(id, "calibration profile id")?;
    let profile = connection.query_row("SELECT id,rule_version,base_rule_version,status,parameters_json,created_at,decided_at,decision_reason_code FROM situation_calibration_profiles WHERE id=?1", [id], profile_row).map_err(crate::database_error)?;
    validate_profile(&profile)?;
    Ok(profile)
}
pub fn profile_by_status(
    connection: &Connection,
    status: &str,
) -> Result<Option<CalibrationProfile>, String> {
    if !matches!(
        status,
        "candidate" | "active" | "superseded" | "rejected" | "rolled-back"
    ) {
        return Err("Invalid calibration profile status".to_string());
    }
    let profile = connection.query_row("SELECT id,rule_version,base_rule_version,status,parameters_json,created_at,decided_at,decision_reason_code FROM situation_calibration_profiles WHERE status=?1 ORDER BY CAST(created_at AS INTEGER) DESC, rowid DESC LIMIT 1", [status], profile_row).optional().map_err(crate::database_error)?;
    if let Some(profile) = &profile {
        validate_profile(profile)?;
    }
    Ok(profile)
}
pub fn candidates(connection: &Connection) -> Result<Vec<CalibrationProfile>, String> {
    let mut s=connection.prepare("SELECT id,rule_version,base_rule_version,status,parameters_json,created_at,decided_at,decision_reason_code FROM situation_calibration_profiles ORDER BY CAST(created_at AS INTEGER) DESC, rowid DESC LIMIT 20").map_err(crate::database_error)?;
    let rows = s
        .query_map([], profile_row)
        .map_err(crate::database_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(crate::database_error)?;
    for profile in &rows {
        validate_profile(profile)?;
    }
    Ok(rows)
}
fn profile_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<CalibrationProfile> {
    let json: String = row.get(4)?;
    let parameters = serde_json::from_str(&json).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(4, rusqlite::types::Type::Text, Box::new(e))
    })?;
    Ok(CalibrationProfile {
        id: row.get(0)?,
        rule_version: row.get(1)?,
        base_rule_version: row.get(2)?,
        status: row.get(3)?,
        parameters,
        created_at: row.get(5)?,
        decided_at: row.get(6)?,
        decision_reason_code: row.get(7)?,
    })
}
pub fn create_candidate(
    connection: &Connection,
    parameters: CalibrationParameters,
) -> Result<CalibrationProfile, String> {
    validate_calibration_parameters(&parameters)?;
    let active = active_profile(connection)?;
    let id = crate::new_id("profile");
    let p = CalibrationProfile {
        rule_version: format!("mvp15-{}", id.trim_start_matches("profile_")),
        id,
        base_rule_version: Some(active.rule_version),
        status: "candidate".into(),
        parameters,
        created_at: crate::now_iso(),
        decided_at: None,
        decision_reason_code: None,
    };
    connection.execute("INSERT INTO situation_calibration_profiles(id,rule_version,base_rule_version,status,parameters_json,created_at) VALUES(?1,?2,?3,?4,?5,?6)",params![p.id,p.rule_version,p.base_rule_version,p.status,serde_json::to_string(&p.parameters).map_err(|e|e.to_string())?,p.created_at]).map_err(crate::database_error)?;
    Ok(p)
}
pub fn save_run(
    connection: &Connection,
    profile_id: &str,
    status: &str,
    metrics: Option<String>,
    error: Option<String>,
) -> Result<CalibrationRun, String> {
    crate::validate_identifier(profile_id, "calibration profile id")?;
    if !matches!(status, "completed" | "failed") {
        return Err("Invalid calibration run status".to_string());
    }
    let profile = profile_by_id(connection, profile_id)?;
    if profile.status != "candidate" {
        return Err("Only candidate profiles can be replayed".to_string());
    }
    if let Some(metrics) = &metrics {
        if metrics.len() > 8_192 || serde_json::from_str::<serde_json::Value>(metrics).is_err() {
            return Err("Invalid calibration metrics".to_string());
        }
    }
    if let Some(error) = &error {
        if error.is_empty()
            || error.len() > 80
            || !error
                .chars()
                .all(|character| character.is_ascii_alphanumeric() || character == '-')
        {
            return Err("Invalid calibration error code".to_string());
        }
    }
    let now = crate::now_iso();
    let run = CalibrationRun {
        id: crate::new_id("calibration_run"),
        profile_id: profile_id.into(),
        fixture_set_version: FIXTURE_SET_VERSION.into(),
        status: status.into(),
        metrics_json: metrics,
        error_code: error,
        started_at: now.clone(),
        completed_at: now,
    };
    validate_run(&run)?;
    let transaction = connection
        .unchecked_transaction()
        .map_err(crate::database_error)?;
    transaction.execute("INSERT INTO situation_calibration_runs(id,profile_id,fixture_set_version,status,metrics_json,error_code,started_at,completed_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",params![run.id,run.profile_id,run.fixture_set_version,run.status,run.metrics_json,run.error_code,run.started_at,run.completed_at]).map_err(crate::database_error)?;
    transaction
        .execute(
            "DELETE FROM situation_calibration_runs WHERE rowid IN (
               SELECT rowid FROM situation_calibration_runs
               ORDER BY CAST(completed_at AS INTEGER) DESC, rowid DESC
               LIMIT -1 OFFSET 100
             )",
            [],
        )
        .map_err(crate::database_error)?;
    transaction.commit().map_err(crate::database_error)?;
    Ok(run)
}
pub fn latest_run(connection: &Connection) -> Result<Option<CalibrationRun>, String> {
    let run = connection.query_row("SELECT id,profile_id,fixture_set_version,status,metrics_json,error_code,started_at,completed_at FROM situation_calibration_runs ORDER BY CAST(completed_at AS INTEGER) DESC, rowid DESC LIMIT 1",[],|r|Ok(CalibrationRun{id:r.get(0)?,profile_id:r.get(1)?,fixture_set_version:r.get(2)?,status:r.get(3)?,metrics_json:r.get(4)?,error_code:r.get(5)?,started_at:r.get(6)?,completed_at:r.get(7)?})).optional().map_err(crate::database_error)?;
    if let Some(run) = &run {
        validate_run(run)?;
    }
    Ok(run)
}
pub fn decide(
    connection: &mut Connection,
    id: &str,
    decision: &str,
    reason: &str,
) -> Result<CalibrationProfile, String> {
    crate::validate_identifier(id, "calibration profile id")?;
    if !matches!(decision, "accept" | "reject" | "rollback") {
        return Err("Invalid calibration decision".into());
    };
    validate_decision_reason(reason)?;
    let tx = connection.transaction().map_err(crate::database_error)?;
    if decision == "accept" {
        let p = profile_by_id(&tx, id)?;
        if p.status != "candidate" {
            return Err("Only candidate profiles can be accepted".into());
        };
        let done:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM situation_calibration_runs WHERE profile_id=?1 AND status='completed')",[id],|r|r.get(0)).map_err(crate::database_error)?;
        if !done {
            return Err("Run calibration before accepting a candidate".into());
        };
        tx.execute("UPDATE situation_calibration_profiles SET status='superseded',decided_at=?1 WHERE status='active'",[crate::now_iso()]).map_err(crate::database_error)?;
        let changed = tx.execute("UPDATE situation_calibration_profiles SET status='active',decided_at=?1,decision_reason_code=?2 WHERE id=?3 AND status='candidate'",params![crate::now_iso(),reason,id]).map_err(crate::database_error)?;
        if changed != 1 {
            return Err("Calibration candidate changed before it could be accepted".to_string());
        }
    } else if decision == "reject" {
        let changed = tx.execute("UPDATE situation_calibration_profiles SET status='rejected',decided_at=?1,decision_reason_code=?2 WHERE id=?3 AND status='candidate'",params![crate::now_iso(),reason,id]).map_err(crate::database_error)?;
        if changed != 1 {
            return Err("Only candidate profiles can be rejected".to_string());
        }
    } else {
        let active = active_profile(&tx)?;
        let previous = profile_by_status(&tx, "superseded")?
            .ok_or_else(|| "No profile is available to roll back to".to_string())?;
        if previous.id != id {
            return Err("Rollback target is not the latest superseded profile".to_string());
        }
        tx.execute("UPDATE situation_calibration_profiles SET status='rolled-back',decided_at=?1,decision_reason_code=?2 WHERE id=?3",params![crate::now_iso(),reason,active.id]).map_err(crate::database_error)?;
        tx.execute(
            "UPDATE situation_calibration_profiles SET status='active',decided_at=?1 WHERE id=?2",
            params![crate::now_iso(), previous.id],
        )
        .map_err(crate::database_error)?;
    }
    tx.commit().map_err(crate::database_error)?;
    active_profile(connection)
}

fn validate_profile(profile: &CalibrationProfile) -> Result<(), String> {
    crate::validate_identifier(&profile.id, "calibration profile id")?;
    if profile.rule_version.is_empty()
        || profile.rule_version.len() > 160
        || !profile.rule_version.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.')
        })
        || !matches!(
            profile.status.as_str(),
            "candidate" | "active" | "superseded" | "rejected" | "rolled-back"
        )
        || profile.created_at.parse::<u128>().is_err()
        || profile
            .decided_at
            .as_ref()
            .is_some_and(|timestamp| timestamp.parse::<u128>().is_err())
        || (profile.status == "candidate"
            && (profile.decided_at.is_some() || profile.decision_reason_code.is_some()))
        || (profile.status != "candidate" && profile.decided_at.is_none())
    {
        return Err("Invalid calibration profile".to_string());
    }
    if let Some(base) = &profile.base_rule_version {
        validate_rule_version(base)?;
    }
    if let Some(reason) = &profile.decision_reason_code {
        validate_decision_reason(reason)?;
    }
    validate_calibration_parameters(&profile.parameters)
}

fn validate_run(run: &CalibrationRun) -> Result<(), String> {
    crate::validate_identifier(&run.id, "calibration run id")?;
    crate::validate_identifier(&run.profile_id, "calibration profile id")?;
    if run.fixture_set_version != FIXTURE_SET_VERSION
        || !matches!(run.status.as_str(), "completed" | "failed")
        || (run.status == "completed" && (run.metrics_json.is_none() || run.error_code.is_some()))
        || (run.status == "failed" && run.error_code.is_none())
        || run.started_at.parse::<u128>().is_err()
        || run.completed_at.parse::<u128>().is_err()
        || run
            .started_at
            .parse::<u128>()
            .ok()
            .zip(run.completed_at.parse::<u128>().ok())
            .is_some_and(|(started, completed)| started > completed)
        || run.metrics_json.as_ref().is_some_and(|metrics| {
            metrics.len() > 8_192 || serde_json::from_str::<serde_json::Value>(metrics).is_err()
        })
        || run.error_code.as_ref().is_some_and(|error| {
            error.is_empty()
                || error.len() > 80
                || !error
                    .chars()
                    .all(|character| character.is_ascii_alphanumeric() || character == '-')
        })
    {
        return Err("Invalid calibration run".to_string());
    }
    Ok(())
}

fn validate_rule_version(version: &str) -> Result<(), String> {
    if version.is_empty()
        || version.len() > 160
        || !version.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.')
        })
    {
        return Err("Invalid calibration rule version".to_string());
    }
    Ok(())
}

fn validate_decision_reason(reason: &str) -> Result<(), String> {
    if matches!(
        reason,
        "wrong-scene"
            | "stale-signal"
            | "unstable-transition"
            | "unwanted-suggestion"
            | "missed-meeting-candidate"
            | "insufficient-evidence"
    ) {
        Ok(())
    } else {
        Err("Invalid calibration decision reason".to_string())
    }
}

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
            serde_json::from_str(include_str!("../../fixtures/situation/mvp1-v1.json"))
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
