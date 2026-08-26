use super::contracts::{validate_calibration_parameters, CalibrationParameters};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::json;

pub const FIXTURE_SET_VERSION: &str = "situation-fixtures-v1";

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ReplaySample {
    elapsed_ms: u64,
    signals: super::contracts::SignalSnapshot,
    expected_scene: String,
}

pub fn replay_metrics(profile: &CalibrationProfile) -> Result<String, String> {
    let samples: Vec<ReplaySample> =
        serde_json::from_str(include_str!("../../fixtures/situation/mvp1-v1.json"))
            .map_err(|_| "Invalid situation replay fixture".to_string())?;
    if samples.is_empty() || samples.len() > 14_400 {
        return Err("Invalid situation replay fixture size".to_string());
    }
    let mut matches = 0_u64;
    for sample in &samples {
        if sample.elapsed_ms > 86_400_000 {
            return Err("Invalid situation replay elapsed time".to_string());
        }
        if super::classifier::classify_with_parameters(&sample.signals, &profile.parameters).scene
            == sample.expected_scene
        {
            matches += 1;
        }
    }
    Ok(json!({"fixtureSetVersion":FIXTURE_SET_VERSION,"profileRuleVersion":profile.rule_version,"sampleCount":samples.len(),"expectedSceneMatches":matches,"deterministic":true}).to_string())
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
    connection.query_row("SELECT id,rule_version,base_rule_version,status,parameters_json,created_at,decided_at,decision_reason_code FROM situation_calibration_profiles WHERE id=?1", [id], profile_row).map_err(crate::database_error)
}
pub fn profile_by_status(
    connection: &Connection,
    status: &str,
) -> Result<Option<CalibrationProfile>, String> {
    connection.query_row("SELECT id,rule_version,base_rule_version,status,parameters_json,created_at,decided_at,decision_reason_code FROM situation_calibration_profiles WHERE status=?1 ORDER BY created_at DESC LIMIT 1", [status], profile_row).optional().map_err(crate::database_error)
}
pub fn candidates(connection: &Connection) -> Result<Vec<CalibrationProfile>, String> {
    let mut s=connection.prepare("SELECT id,rule_version,base_rule_version,status,parameters_json,created_at,decided_at,decision_reason_code FROM situation_calibration_profiles ORDER BY created_at DESC LIMIT 20").map_err(crate::database_error)?;
    let rows = s
        .query_map([], profile_row)
        .map_err(crate::database_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(crate::database_error)?;
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
    let p = CalibrationProfile {
        id: crate::new_id("profile"),
        rule_version: format!("mvp15-{}", crate::now_iso()),
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
    connection.execute("INSERT INTO situation_calibration_runs(id,profile_id,fixture_set_version,status,metrics_json,error_code,started_at,completed_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",params![run.id,run.profile_id,run.fixture_set_version,run.status,run.metrics_json,run.error_code,run.started_at,run.completed_at]).map_err(crate::database_error)?;
    Ok(run)
}
pub fn latest_run(connection: &Connection) -> Result<Option<CalibrationRun>, String> {
    connection.query_row("SELECT id,profile_id,fixture_set_version,status,metrics_json,error_code,started_at,completed_at FROM situation_calibration_runs ORDER BY completed_at DESC LIMIT 1",[],|r|Ok(CalibrationRun{id:r.get(0)?,profile_id:r.get(1)?,fixture_set_version:r.get(2)?,status:r.get(3)?,metrics_json:r.get(4)?,error_code:r.get(5)?,started_at:r.get(6)?,completed_at:r.get(7)?})).optional().map_err(crate::database_error)
}
pub fn decide(
    connection: &mut Connection,
    id: &str,
    decision: &str,
    reason: &str,
) -> Result<CalibrationProfile, String> {
    if !matches!(decision, "accept" | "reject" | "rollback") {
        return Err("Invalid calibration decision".into());
    };
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
        tx.execute("UPDATE situation_calibration_profiles SET status='active',decided_at=?1,decision_reason_code=?2 WHERE id=?3",params![crate::now_iso(),reason,id]).map_err(crate::database_error)?;
    } else if decision == "reject" {
        tx.execute("UPDATE situation_calibration_profiles SET status='rejected',decided_at=?1,decision_reason_code=?2 WHERE id=?3 AND status='candidate'",params![crate::now_iso(),reason,id]).map_err(crate::database_error)?;
    } else {
        let active = active_profile(&tx)?;
        let previous = profile_by_status(&tx, "superseded")?
            .ok_or_else(|| "No profile is available to roll back to".to_string())?;
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
