//! Local-only, incremental learning-data materialization.
//!
//! A routing decision is immutable when it is written.  This module snapshots that decision and
//! explicit user feedback into a dataset. It never calls an LLM, so a scheduler can run it
//! safely in the background without changing a live routing decision.
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

const FEATURE_VERSION: &str = "rr-features-v1";
const LABELER_VERSION: &str = "rr-labeler-v1";

pub(crate) fn mark_root_dirty(connection: &Connection, root_id: &str) -> Result<(), String> {
    let cause_seq: i64 = connection
        .query_row(
            "SELECT COALESCE(MAX(seq), 1) FROM rr_events WHERE root_id=?1",
            [root_id],
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())?;
    connection
        .execute(
            "INSERT INTO rr_learning_dirty(root_id,cause_seq) VALUES(?1,?2)
         ON CONFLICT(root_id) DO UPDATE SET cause_seq=MAX(cause_seq,excluded.cause_seq)",
            params![root_id, cause_seq],
        )
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// Converts a stable prefix of the dirty queue to one immutable dataset. Roots inserted after
/// `upper_rowid` remain in the queue for the next run; no event can be lost at the boundary.
pub(crate) fn materialize_dirty_roots(
    connection: &mut Connection,
    now_ms: i64,
    batch_size: u16,
) -> Result<Option<String>, String> {
    let upper_rowid: i64 = connection
        .query_row(
            "SELECT COALESCE(MAX(rowid),0) FROM rr_learning_dirty",
            [],
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())?;
    if upper_rowid == 0 {
        return Ok(None);
    }
    let dataset_id = format!("rr-dataset-{now_ms}-{upper_rowid}");
    let job_id = format!("rr-learning-extract-{upper_rowid}");
    let tx = connection
        .unchecked_transaction()
        .map_err(|e| e.to_string())?;
    tx.execute("INSERT OR IGNORE INTO rr_datasets(id,upper_seq,feature_version,labeler_version,manifest_json,state,created_at_ms) VALUES(?1,?2,?3,?4,'{}','building',?5)", params![dataset_id,upper_rowid,FEATURE_VERSION,LABELER_VERSION,now_ms]).map_err(|e| e.to_string())?;
    tx.execute("INSERT OR IGNORE INTO rr_learning_jobs(id,job_key,stage,status,upper_seq,cursor_seq,dataset_id,created_at_ms,updated_at_ms) VALUES(?1,?2,'extract','running',?3,0,?4,?5,?5)", params![job_id,format!("extract:{upper_rowid}"),upper_rowid,dataset_id,now_ms]).map_err(|e| e.to_string())?;
    let roots = {
        let mut stmt = tx.prepare("SELECT rowid,root_id FROM rr_learning_dirty WHERE rowid<=?1 ORDER BY rowid LIMIT ?2").map_err(|e| e.to_string())?;
        let roots = stmt
            .query_map(params![upper_rowid, i64::from(batch_size)], |r| {
                Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?))
            })
            .map_err(|e| e.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?;
        roots
    };
    for (rowid, root_id) in &roots {
        materialize_root(&tx, &dataset_id, root_id, now_ms)?;
        tx.execute(
            "DELETE FROM rr_learning_dirty WHERE root_id=?1 AND rowid=?2",
            params![root_id, rowid],
        )
        .map_err(|e| e.to_string())?;
    }
    let count: i64 = tx
        .query_row(
            "SELECT count(*) FROM rr_examples WHERE dataset_id=?1",
            [&dataset_id],
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())?;
    let manifest = json!({"datasetId":dataset_id,"upperDirtyRowid":upper_rowid,"featureVersion":FEATURE_VERSION,"labelerVersion":LABELER_VERSION,"exampleCount":count,"rootCount":roots.len()});
    tx.execute(
        "UPDATE rr_datasets SET manifest_json=?1,digest=?2,state='ready' WHERE id=?3",
        params![
            manifest.to_string(),
            sha256(manifest.to_string().as_bytes()),
            dataset_id
        ],
    )
    .map_err(|e| e.to_string())?;
    tx.execute(
        "UPDATE rr_learning_jobs SET status='completed',cursor_seq=?1,updated_at_ms=?2 WHERE id=?3",
        params![upper_rowid, now_ms, job_id],
    )
    .map_err(|e| e.to_string())?;
    tx.commit().map_err(|e| e.to_string())?;
    Ok(Some(dataset_id))
}

fn materialize_root(
    connection: &Connection,
    dataset_id: &str,
    root_id: &str,
    now_ms: i64,
) -> Result<(), String> {
    let decision: Option<(String,String,String)> = connection.query_row("SELECT id,features_json,selected_id FROM rr_decisions WHERE root_id=?1 ORDER BY revision DESC,created_at_ms DESC LIMIT 1", [root_id], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional().map_err(|e| e.to_string())?;
    let Some((decision_id, features_json, selected_id)) = decision else {
        return Ok(());
    };
    let features: Value = serde_json::from_str(&features_json)
        .map_err(|_| "Stored routing features are invalid".to_string())?;
    let feedback: Option<String> = connection.query_row("SELECT kind FROM rr_feedback WHERE target_root_id=?1 ORDER BY created_at_ms DESC LIMIT 1", [root_id], |r| r.get(0)).optional().map_err(|e| e.to_string())?;
    let (outcome, eligible, reason) = match feedback.as_deref() {
        Some("explicit_positive") => ("positive", 1, None),
        Some("explicit_negative") | Some("answer_challenge") => ("negative", 1, None),
        Some(_) => ("unknown", 0, Some("unsupported_feedback")),
        None => ("unknown", 0, Some("no_explicit_feedback")),
    };
    let id = format!(
        "rr-example-{}",
        &sha256(format!("{dataset_id}:{decision_id}:1").as_bytes())[..24]
    );
    let labels = json!({"outcome":outcome,"selectedRecipeId":selected_id,"feedbackKind":feedback});
    connection.execute("INSERT OR IGNORE INTO rr_examples(id,dataset_id,decision_id,label_revision,feature_version,labeler_version,features_json,labels_json,eligible,exclusion_reason,created_at_ms) VALUES(?1,?2,?3,1,?4,?5,?6,?7,?8,?9,?10)",params![id,dataset_id,decision_id,FEATURE_VERSION,LABELER_VERSION,features.to_string(),labels.to_string(),eligible,reason,now_ms]).map_err(|e| e.to_string())?;
    connection.execute("INSERT OR IGNORE INTO rr_example_sources(example_id,source_kind,source_id,source_version,scope_key) VALUES(?1,'decision',?2,'1',?3)",params![id,decision_id,root_id]).map_err(|e| e.to_string())?;
    connection.execute("INSERT OR IGNORE INTO rr_example_sources(example_id,source_kind,source_id,source_version,scope_key) SELECT ?1,'feedback',id,extractor_version,?2 FROM rr_feedback WHERE target_root_id=?2",params![id,root_id]).map_err(|e| e.to_string())?;
    Ok(())
}

pub(crate) fn invalidate_artifacts_for_dataset(
    connection: &Connection,
    dataset_id: &str,
) -> Result<usize, String> {
    connection.execute("UPDATE rr_ranker_artifacts SET state='invalidated' WHERE dataset_id=?1 AND state IN ('candidate','shadow')",[dataset_id]).map_err(|e| e.to_string())
}

/// Publishes only a shadow artifact. Execution selection remains rules-controlled until a
/// separate policy change names the artifact, which makes retrospective evaluation possible.
pub(crate) fn publish_shadow_artifact(
    connection: &Connection,
    dataset_id: &str,
    candidate_fingerprint: &str,
    weights_json: &str,
    metrics_json: &str,
    now_ms: i64,
) -> Result<String, String> {
    let ready: bool = connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM rr_datasets WHERE id=?1 AND state='ready')",
            [dataset_id],
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())?;
    if !ready {
        return Err("Learning dataset is not ready".into());
    }
    let weights: Value = serde_json::from_str(weights_json)
        .map_err(|_| "Ranker weights are invalid JSON".to_string())?;
    let metrics: Value = serde_json::from_str(metrics_json)
        .map_err(|_| "Ranker metrics are invalid JSON".to_string())?;
    if !weights.is_object() || !metrics.is_object() {
        return Err("Ranker artifact must contain JSON objects".into());
    }
    let normalized_weights = weights.to_string();
    let normalized_metrics = metrics.to_string();
    let digest = sha256(
        format!("{dataset_id}:{candidate_fingerprint}:{normalized_weights}:{normalized_metrics}")
            .as_bytes(),
    );
    let id = format!("rr-ranker-{}", &digest[..24]);
    connection.execute("INSERT OR IGNORE INTO rr_ranker_artifacts(id,dataset_id,algorithm,feature_version,candidate_fingerprint,weights_json,metrics_json,digest,state,created_at_ms) VALUES(?1,?2,'empirical-v1',?3,?4,?5,?6,?7,'shadow',?8)",params![id,dataset_id,FEATURE_VERSION,candidate_fingerprint,normalized_weights,normalized_metrics,digest,now_ms]).map_err(|e|e.to_string())?;
    Ok(id)
}
fn sha256(input: &[u8]) -> String {
    format!("{:x}", Sha256::digest(input))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rr_28_materialization_is_incremental_and_explicit() {
        let mut c = Connection::open_in_memory().expect("db");
        c.execute_batch("PRAGMA foreign_keys=ON;CREATE TABLE conversations(id TEXT PRIMARY KEY);CREATE TABLE runtime_runs(id TEXT PRIMARY KEY);CREATE TABLE conversation_messages(id TEXT PRIMARY KEY,conversation_id TEXT,role TEXT,content TEXT,created_at TEXT);INSERT INTO conversations VALUES('c');").expect("base");
        crate::role_routing::schema::migrate(&c).expect("routing");
        super::super::schema::migrate(&c).expect("learning");
        c.execute(
            "INSERT INTO rr_policy_versions VALUES('p',1,'{}','d',1)",
            [],
        )
        .expect("policy");
        c.execute("INSERT INTO rr_roots(root_id,conversation_id,policy_id,phase,origin,presentation_mode,started_at_ms,scope_digest) VALUES('r','c','p','completed','text','visual',1,'')",[]).expect("root");
        c.execute("INSERT INTO rr_decisions VALUES('d','r',0,NULL,'{\"complexity\":2}','[]','qwen','respond','[]','rules-v1','p',1)",[]).expect("decision");
        c.execute(
            "INSERT INTO rr_events VALUES('r',1,'answer_committed','{}',1)",
            [],
        )
        .expect("event");
        mark_root_dirty(&c, "r").expect("dirty");
        let dataset = materialize_dirty_roots(&mut c, 10, 10)
            .expect("run")
            .expect("dataset");
        let row: (i64, String) = c
            .query_row(
                "SELECT eligible,exclusion_reason FROM rr_examples WHERE dataset_id=?1",
                [dataset],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .expect("example");
        assert_eq!(row, (0, "no_explicit_feedback".into()));
        assert!(materialize_dirty_roots(&mut c, 11, 10)
            .expect("again")
            .is_none());
    }

    #[test]
    fn rr_35_artifact_is_shadow_only_and_requires_a_ready_dataset() {
        let c = Connection::open_in_memory().expect("db");
        c.execute_batch("CREATE TABLE rr_roots(root_id TEXT PRIMARY KEY);CREATE TABLE rr_decisions(id TEXT PRIMARY KEY);").expect("base");
        super::super::schema::migrate(&c).expect("schema");
        assert!(publish_shadow_artifact(&c, "missing", "candidates", "{}", "{}", 1).is_err());
        c.execute("INSERT INTO rr_datasets(id,upper_seq,feature_version,labeler_version,manifest_json,state,created_at_ms) VALUES('d',0,'f','l','{}','ready',1)",[]).expect("dataset");
        let id = publish_shadow_artifact(&c, "d", "candidates", "{}", "{}", 2).expect("artifact");
        assert_eq!(
            c.query_row(
                "SELECT state FROM rr_ranker_artifacts WHERE id=?1",
                [id],
                |r| r.get::<_, String>(0)
            )
            .expect("state"),
            "shadow"
        );
    }
}
