//! Host-generated execution evidence. Model prose and job IDs are not proof.
use crate::database_error;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

pub(crate) const SCHEMA_VERSION: i64 = 1;
pub(crate) const PRODUCER_HOST_PI: &str = "host_pi";
pub(crate) const PRODUCER_HOST_RECIPE: &str = "host_recipe";
pub(crate) const PRODUCER_LEGACY: &str = "legacy_unverified";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ExecutionEvidence {
    pub schema_version: i64,
    pub task_id: String,
    pub job_id: String,
    pub run_id: String,
    pub recipe_id: Option<String>,
    pub recipe_revision: Option<i64>,
    pub recipe_digest: Option<String>,
    pub target_digest: String,
    pub terminal_kind: String,
    pub exit_code: Option<i64>,
    pub result_ref: Option<String>,
    pub result_digest: Option<String>,
    pub producer: String,
    pub readable: bool,
    pub reason_code: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum EvidenceStatus {
    Valid,
    Missing(&'static str),
    Unknown(&'static str),
}

pub(crate) fn digest_bytes(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

pub(crate) fn digest_text(text: &str) -> String {
    digest_bytes(text.as_bytes())
}

pub(crate) fn path_within_root(root: &Path, candidate: &Path) -> Result<PathBuf, String> {
    let root = std::fs::canonicalize(root).map_err(|_| "target_root_unreadable")?;
    let resolved = if candidate.is_absolute() {
        std::fs::canonicalize(candidate).map_err(|_| "target_unreadable")?
    } else {
        std::fs::canonicalize(root.join(candidate)).map_err(|_| "target_unreadable")?
    };
    if !resolved.starts_with(&root) {
        return Err("target_outside_root".into());
    }
    Ok(resolved)
}

pub(crate) fn validate(evidence: &ExecutionEvidence) -> EvidenceStatus {
    if evidence.schema_version != SCHEMA_VERSION {
        return EvidenceStatus::Unknown("schema_mismatch");
    }
    if evidence.producer == PRODUCER_LEGACY {
        return EvidenceStatus::Unknown("legacy_unverified");
    }
    if !matches!(
        evidence.producer.as_str(),
        PRODUCER_HOST_PI | PRODUCER_HOST_RECIPE
    ) {
        return EvidenceStatus::Unknown("producer_unknown");
    }
    if evidence.job_id.is_empty()
        || evidence.run_id.is_empty()
        || evidence.task_id.is_empty()
        || evidence.target_digest.is_empty()
        || evidence.target_digest.chars().all(|c| c == '0')
    {
        return EvidenceStatus::Missing("identity_incomplete");
    }
    if evidence
        .result_digest
        .as_deref()
        .is_some_and(|d| d.is_empty())
    {
        return EvidenceStatus::Missing("empty_digest");
    }
    if !evidence.readable || evidence.result_ref.is_none() || evidence.result_digest.is_none() {
        return EvidenceStatus::Missing("result_unreadable");
    }
    EvidenceStatus::Valid
}

pub(crate) fn persist(connection: &Connection, evidence: &ExecutionEvidence) -> Result<(), String> {
    if evidence.producer != PRODUCER_LEGACY && validate(evidence) != EvidenceStatus::Valid {
        return Err("evidence_incomplete".into());
    }
    connection
        .execute(
            "INSERT INTO steward_execution_evidence(
               run_id,task_id,job_id,schema_version,recipe_id,recipe_revision,recipe_digest,
               target_digest,terminal_kind,exit_code,result_ref,result_digest,producer,readable,reason_code,payload_json,created_at)
             VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17)
             ON CONFLICT(run_id) DO UPDATE SET
               task_id=excluded.task_id,
               job_id=excluded.job_id,
               schema_version=excluded.schema_version,
               recipe_id=excluded.recipe_id,
               recipe_revision=excluded.recipe_revision,
               recipe_digest=excluded.recipe_digest,
               target_digest=excluded.target_digest,
               terminal_kind=excluded.terminal_kind,
               exit_code=excluded.exit_code,
               result_ref=excluded.result_ref,
               result_digest=excluded.result_digest,
               producer=excluded.producer,
               readable=excluded.readable,
               reason_code=excluded.reason_code,
               payload_json=excluded.payload_json",
            params![
                evidence.run_id,
                evidence.task_id,
                evidence.job_id,
                evidence.schema_version,
                evidence.recipe_id,
                evidence.recipe_revision,
                evidence.recipe_digest,
                evidence.target_digest,
                evidence.terminal_kind,
                evidence.exit_code,
                evidence.result_ref,
                evidence.result_digest,
                evidence.producer,
                i64::from(evidence.readable),
                evidence.reason_code,
                serde_json::to_string(evidence).map_err(|_| "evidence_invalid")?,
                crate::now_iso()
            ],
        )
        .map_err(database_error)?;
    Ok(())
}

pub(crate) fn load_for_run(
    connection: &Connection,
    run_id: &str,
) -> Result<Option<ExecutionEvidence>, String> {
    let payload: Option<String> = connection
        .query_row(
            "SELECT payload_json FROM steward_execution_evidence WHERE run_id=?1",
            [run_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(database_error)?;
    payload
        .map(|text| serde_json::from_str(&text).map_err(|_| "evidence_invalid".to_string()))
        .transpose()
}

#[allow(clippy::too_many_arguments)] // Evidence fields stay explicit at the host-session boundary.
pub(crate) fn from_host_session(
    task_id: &str,
    job_id: &str,
    run_id: &str,
    terminal_kind: &str,
    workspace: &str,
    result_ref: Option<&str>,
    result_body: &str,
    recipe_id: Option<&str>,
    recipe_revision: Option<i64>,
    recipe_digest: Option<&str>,
    exit_code: Option<i64>,
    producer: &str,
) -> ExecutionEvidence {
    let readable = result_ref.is_some() && !result_body.is_empty();
    ExecutionEvidence {
        schema_version: SCHEMA_VERSION,
        task_id: task_id.into(),
        job_id: job_id.into(),
        run_id: run_id.into(),
        recipe_id: recipe_id.map(str::to_string),
        recipe_revision,
        recipe_digest: recipe_digest.map(str::to_string),
        target_digest: digest_text(workspace),
        terminal_kind: terminal_kind.into(),
        exit_code,
        result_ref: result_ref.map(str::to_string),
        result_digest: readable.then(|| digest_text(result_body)),
        producer: producer.into(),
        readable,
        reason_code: if readable {
            "host_observed".into()
        } else {
            "result_unreadable".into()
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn valid() -> ExecutionEvidence {
        ExecutionEvidence {
            schema_version: SCHEMA_VERSION,
            task_id: "t".into(),
            job_id: "j".into(),
            run_id: "r".into(),
            recipe_id: Some("recipe".into()),
            recipe_revision: Some(1),
            recipe_digest: Some("abc".into()),
            target_digest: digest_text("/tmp/ws"),
            terminal_kind: "settled".into(),
            exit_code: Some(0),
            result_ref: Some("out.log".into()),
            result_digest: Some(digest_text("ok")),
            producer: PRODUCER_HOST_RECIPE.into(),
            readable: true,
            reason_code: "host_observed".into(),
        }
    }

    #[test]
    fn rf5_v_01_empty_json_job_only_and_empty_digest_are_not_pass() {
        assert!(matches!(
            validate(&ExecutionEvidence {
                job_id: String::new(),
                ..valid()
            }),
            EvidenceStatus::Missing(_)
        ));
        assert!(matches!(
            validate(&ExecutionEvidence {
                result_digest: Some(String::new()),
                ..valid()
            }),
            EvidenceStatus::Missing(_)
        ));
        assert!(matches!(
            validate(&ExecutionEvidence {
                producer: PRODUCER_LEGACY.into(),
                ..valid()
            }),
            EvidenceStatus::Unknown("legacy_unverified")
        ));
        assert_eq!(validate(&valid()), EvidenceStatus::Valid);
    }

    #[test]
    fn persist_replaces_incomplete_evidence_for_the_same_run() {
        use crate::persistence::schema::initialize_database;
        use rusqlite::Connection;
        let connection = Connection::open_in_memory().unwrap();
        initialize_database(&connection).unwrap();
        let mut first = valid();
        first.exit_code = None;
        first.terminal_kind = "running".into();
        persist(&connection, &first).unwrap();
        persist(&connection, &valid()).unwrap();
        let loaded = load_for_run(&connection, "r").unwrap().unwrap();
        assert_eq!(loaded.terminal_kind, "settled");
        assert_eq!(loaded.exit_code, Some(0));
    }
}
