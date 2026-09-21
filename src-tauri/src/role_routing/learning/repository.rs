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
    if batch_size == 0 {
        return Err("Role-routing learning batch size must be positive".into());
    }
    let tx = connection
        .unchecked_transaction()
        .map_err(|e| e.to_string())?;
    let upper_rowid: i64 = tx
        .query_row(
            "SELECT COALESCE(MAX(rowid),0) FROM rr_learning_dirty",
            [],
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())?;
    if upper_rowid == 0 {
        return Ok(None);
    }
    let roots = {
        let mut stmt = tx.prepare("SELECT rowid,root_id,cause_seq FROM rr_learning_dirty WHERE rowid<=?1 ORDER BY rowid LIMIT ?2").map_err(|e| e.to_string())?;
        let roots = stmt
            .query_map(params![upper_rowid, i64::from(batch_size)], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, i64>(2)?,
                ))
            })
            .map_err(|e| e.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?;
        roots
    };
    let page_lower = roots
        .first()
        .map(|(rowid, _, _)| *rowid)
        .ok_or_else(|| "Role-routing learning dirty page is empty".to_string())?;
    let page_upper = roots
        .last()
        .map(|(rowid, _, _)| *rowid)
        .ok_or_else(|| "Role-routing learning dirty page is empty".to_string())?;
    let page_fingerprint = sha256(
        roots
            .iter()
            .map(|(rowid, root_id, cause_seq)| format!("{rowid}:{root_id}:{cause_seq}"))
            .collect::<Vec<_>>()
            .join("|")
            .as_bytes(),
    );
    let page_key = &page_fingerprint[..24];
    let dataset_id = format!("rr-dataset-{now_ms}-{page_key}");
    let job_id = format!("rr-learning-extract-{now_ms}-{page_key}");
    tx.execute("INSERT INTO rr_datasets(id,upper_seq,feature_version,labeler_version,manifest_json,state,created_at_ms) VALUES(?1,?2,?3,?4,'{}','building',?5)", params![dataset_id,page_upper,FEATURE_VERSION,LABELER_VERSION,now_ms]).map_err(|e| e.to_string())?;
    tx.execute("INSERT INTO rr_learning_jobs(id,job_key,stage,status,upper_seq,cursor_seq,dataset_id,created_at_ms,updated_at_ms) VALUES(?1,?2,'extract','running',?3,0,?4,?5,?5)", params![job_id,format!("extract:{now_ms}:{page_key}"),page_upper,dataset_id,now_ms]).map_err(|e| e.to_string())?;
    for (rowid, root_id, _cause_seq) in &roots {
        materialize_root(&tx, &dataset_id, root_id, now_ms)?;
        let changed = tx
            .execute(
                "DELETE FROM rr_learning_dirty WHERE root_id=?1 AND rowid=?2",
                params![root_id, rowid],
            )
            .map_err(|e| e.to_string())?;
        if changed != 1 {
            return Err(
                "Role-routing learning dirty receipt changed during materialization".into(),
            );
        }
    }
    let count: i64 = tx
        .query_row(
            "SELECT count(*) FROM rr_examples WHERE dataset_id=?1",
            [&dataset_id],
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())?;
    let manifest = json!({"datasetId":dataset_id,"lowerDirtyRowid":page_lower,"upperDirtyRowid":page_upper,"featureVersion":FEATURE_VERSION,"labelerVersion":LABELER_VERSION,"exampleCount":count,"rootCount":roots.len()});
    let dataset_changed = tx
        .execute(
            "UPDATE rr_datasets SET manifest_json=?1,digest=?2,state='ready' WHERE id=?3",
            params![
                manifest.to_string(),
                sha256(manifest.to_string().as_bytes()),
                dataset_id
            ],
        )
        .map_err(|e| e.to_string())?;
    if dataset_changed != 1 {
        return Err("Role-routing learning dataset was not finalized".into());
    }
    let job_changed = tx.execute(
        "UPDATE rr_learning_jobs SET status='completed',cursor_seq=?1,updated_at_ms=?2 WHERE id=?3",
        params![page_upper, now_ms, job_id],
    )
    .map_err(|e| e.to_string())?;
    if job_changed != 1 {
        return Err("Role-routing learning job was not finalized".into());
    }
    tx.commit().map_err(|e| e.to_string())?;
    Ok(Some(dataset_id))
}

fn materialize_root(
    connection: &Connection,
    dataset_id: &str,
    root_id: &str,
    now_ms: i64,
) -> Result<(), String> {
    let decision: Option<(String,String,String,String)> = connection.query_row("SELECT d.id,d.features_json,d.selected_id,r.phase FROM rr_decisions d JOIN rr_roots r ON r.root_id=d.root_id WHERE d.root_id=?1 ORDER BY d.revision DESC,d.created_at_ms DESC LIMIT 1", [root_id], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).optional().map_err(|e| e.to_string())?;
    let Some((decision_id, features_json, selected_id, root_phase)) = decision else {
        return Ok(());
    };
    let features: Value = serde_json::from_str(&features_json)
        .map_err(|_| "Stored routing features are invalid".to_string())?;
    let feedback = connection
        .prepare("SELECT kind FROM rr_feedback WHERE target_root_id=?1 ORDER BY created_at_ms,id")
        .map_err(|e| e.to_string())?
        .query_map([root_id], |row| row.get::<_, String>(0))
        .map_err(|e| e.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;
    let positive = feedback.iter().any(|kind| kind == "explicit_positive");
    let negative = feedback.iter().any(|kind| kind == "explicit_negative");
    let challenge = feedback.iter().any(|kind| kind == "answer_challenge");
    let (outcome, eligible, reason) = if root_phase != "completed" {
        ("unknown", 0, Some("root_not_completed"))
    } else if positive && negative {
        ("unknown", 0, Some("conflicting_explicit_feedback"))
    } else if positive {
        ("positive", 1, None)
    } else if negative {
        ("negative", 1, None)
    } else if challenge {
        ("unknown", 0, Some("challenge_is_not_an_outcome_label"))
    } else if feedback.is_empty() {
        ("unknown", 0, Some("no_explicit_feedback"))
    } else {
        ("unknown", 0, Some("unsupported_feedback"))
    };
    let label_revision = i64::try_from(feedback.len()).unwrap_or(i64::MAX).max(1);
    let id = format!(
        "rr-example-{}",
        &sha256(format!("{dataset_id}:{decision_id}:{label_revision}").as_bytes())[..24]
    );
    let labels = json!({"outcome":outcome,"selectedRecipeId":selected_id,"feedbackKinds":feedback});
    connection.execute("INSERT OR IGNORE INTO rr_examples(id,dataset_id,decision_id,label_revision,feature_version,labeler_version,features_json,labels_json,eligible,exclusion_reason,created_at_ms) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)",params![id,dataset_id,decision_id,label_revision,FEATURE_VERSION,LABELER_VERSION,features.to_string(),labels.to_string(),eligible,reason,now_ms]).map_err(|e| e.to_string())?;
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
    let eligible_examples: i64 = connection
        .query_row(
            "SELECT count(*) FROM rr_examples WHERE dataset_id=?1 AND eligible=1",
            [dataset_id],
            |row| row.get(0),
        )
        .map_err(|error| error.to_string())?;
    if eligible_examples < 20 {
        return Err("Learning dataset has insufficient eligible examples".into());
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

    fn materialized_label(feedback: &[&str], phase: &str) -> (String, i64, String, i64) {
        let mut connection = Connection::open_in_memory().expect("db");
        connection.execute_batch("PRAGMA foreign_keys=ON;CREATE TABLE conversations(id TEXT PRIMARY KEY);CREATE TABLE runtime_runs(id TEXT PRIMARY KEY);CREATE TABLE conversation_messages(id TEXT PRIMARY KEY,conversation_id TEXT,role TEXT,content TEXT,created_at TEXT);INSERT INTO conversations VALUES('c');INSERT INTO conversation_messages VALUES('answer','c','assistant','answer','1');").expect("base");
        crate::role_routing::schema::migrate(&connection).expect("routing");
        super::super::schema::migrate(&connection).expect("learning");
        connection
            .execute(
                "INSERT INTO rr_policy_versions VALUES('p',1,'{}','d',1)",
                [],
            )
            .expect("policy");
        connection.execute("INSERT INTO rr_roots(root_id,conversation_id,policy_id,phase,origin,presentation_mode,started_at_ms,scope_digest,result_message_id) VALUES('r','c','p',?1,'text','visual',1,'','answer')", [phase]).expect("root");
        connection.execute("INSERT INTO rr_decisions VALUES('d','r',0,NULL,'{\"complexity\":2}','[]','qwen','respond','[]','rules-v1','p',1)", []).expect("decision");
        connection
            .execute(
                "INSERT INTO rr_events VALUES('r',1,'answer_committed','{}',1)",
                [],
            )
            .expect("event");
        for (index, kind) in feedback.iter().enumerate() {
            let source = format!("source-{index}");
            connection
                .execute(
                    "INSERT INTO conversation_messages VALUES(?1,'c','user','feedback',?2)",
                    params![source, (index + 2).to_string()],
                )
                .expect("source");
            connection.execute("INSERT INTO rr_feedback(id,target_answer_id,target_root_id,source_message_id,kind,evidence_json,label_source,confidence,extractor_version,status,created_at_ms) VALUES(?1,'answer','r',?2,?3,'{}','host',1.0,'host-v1','recorded',?4)", params![format!("feedback-{index}"), source, kind, index as i64 + 2]).expect("feedback");
        }
        mark_root_dirty(&connection, "r").expect("dirty");
        let dataset = materialize_dirty_roots(&mut connection, 20, 10)
            .expect("materialize")
            .expect("dataset");
        connection.query_row(
            "SELECT json_extract(labels_json,'$.outcome'),eligible,exclusion_reason,label_revision FROM rr_examples WHERE dataset_id=?1",
            [dataset],
            |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?)),
        ).expect("label")
    }

    #[test]
    fn rr_32_silence_and_challenge_are_not_success_or_failure() {
        assert_eq!(
            materialized_label(&[], "completed"),
            ("unknown".into(), 0, "no_explicit_feedback".into(), 1)
        );
        assert_eq!(
            materialized_label(&["answer_challenge"], "completed"),
            (
                "unknown".into(),
                0,
                "challenge_is_not_an_outcome_label".into(),
                1
            )
        );
    }

    #[test]
    fn rr_32_cancel_not_failure_and_conflict_is_excluded() {
        assert_eq!(
            materialized_label(&["explicit_negative"], "cancelled"),
            ("unknown".into(), 0, "root_not_completed".into(), 1)
        );
        assert_eq!(
            materialized_label(&["explicit_positive", "explicit_negative"], "completed"),
            (
                "unknown".into(),
                0,
                "conflicting_explicit_feedback".into(),
                2
            )
        );
    }
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
        c.execute("INSERT INTO rr_roots(root_id,conversation_id,policy_id,phase,origin,presentation_mode,started_at_ms,scope_digest) VALUES('r2','c','p','completed','text','visual',2,'')",[]).expect("second root");
        c.execute("INSERT INTO rr_decisions VALUES('d2','r2',0,NULL,'{\"complexity\":1}','[]','qwen','respond','[]','rules-v1','p',2)",[]).expect("second decision");
        c.execute(
            "INSERT INTO rr_events VALUES('r2',1,'answer_committed','{}',2)",
            [],
        )
        .expect("second event");
        mark_root_dirty(&c, "r").expect("dirty");
        mark_root_dirty(&c, "r2").expect("second dirty");
        assert!(materialize_dirty_roots(&mut c, 9, 0).is_err());
        let dataset = materialize_dirty_roots(&mut c, 10, 1)
            .expect("run")
            .expect("dataset");
        let row: (i64, String) = c
            .query_row(
                "SELECT eligible,exclusion_reason FROM rr_examples WHERE dataset_id=?1",
                [&dataset],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .expect("example");
        assert_eq!(row, (0, "no_explicit_feedback".into()));
        assert_eq!(
            c.query_row("SELECT count(*) FROM rr_learning_dirty", [], |r| r
                .get::<_, i64>(0))
                .expect("one dirty remains"),
            1
        );
        let second_dataset = materialize_dirty_roots(&mut c, 11, 1)
            .expect("second page")
            .expect("second dataset");
        assert_ne!(dataset, second_dataset);
        assert!(materialize_dirty_roots(&mut c, 12, 1)
            .expect("again")
            .is_none());
        assert_eq!(
            c.query_row(
                "SELECT count(*) FROM rr_learning_jobs j JOIN rr_datasets d ON d.id=j.dataset_id WHERE j.status='completed' AND j.cursor_seq=j.upper_seq AND d.state='ready'",
                [],
                |r| r.get::<_, i64>(0)
            )
            .expect("completed pages"),
            2
        );
    }

    #[test]
    fn rr_31_crash_before_checkpoint_keeps_the_page_retryable() {
        let mut connection = Connection::open_in_memory().expect("db");
        connection.execute_batch("PRAGMA foreign_keys=ON;CREATE TABLE conversations(id TEXT PRIMARY KEY);CREATE TABLE runtime_runs(id TEXT PRIMARY KEY);CREATE TABLE conversation_messages(id TEXT PRIMARY KEY,conversation_id TEXT,role TEXT,content TEXT,created_at TEXT);INSERT INTO conversations VALUES('c');").expect("base");
        crate::role_routing::schema::migrate(&connection).expect("routing");
        super::super::schema::migrate(&connection).expect("learning");
        connection
            .execute(
                "INSERT INTO rr_policy_versions VALUES('p',1,'{}','d',1)",
                [],
            )
            .expect("policy");
        connection.execute("INSERT INTO rr_roots(root_id,conversation_id,policy_id,phase,origin,presentation_mode,started_at_ms,scope_digest) VALUES('r','c','p','completed','text','visual',1,'')", []).expect("root");
        connection.execute("INSERT INTO rr_decisions VALUES('d','r',0,NULL,'{\"quality\":null}','[]','qwen','respond','[]','rules-v1','p',1)", []).expect("decision");
        connection
            .execute(
                "INSERT INTO rr_events VALUES('r',1,'answer_committed','{}',1)",
                [],
            )
            .expect("event");
        mark_root_dirty(&connection, "r").expect("dirty");
        connection.execute_batch("CREATE TRIGGER fail_learning_page BEFORE INSERT ON rr_examples BEGIN SELECT RAISE(ABORT,'fixture crash'); END;").expect("trigger");
        assert!(materialize_dirty_roots(&mut connection, 10, 10).is_err());
        assert_eq!(
            connection
                .query_row("SELECT count(*) FROM rr_learning_dirty", [], |row| row
                    .get::<_, i64>(0))
                .expect("dirty count"),
            1
        );
        assert_eq!(
            connection
                .query_row("SELECT count(*) FROM rr_datasets", [], |row| row
                    .get::<_, i64>(0))
                .expect("dataset count"),
            0
        );
        assert_eq!(
            connection
                .query_row("SELECT count(*) FROM rr_learning_jobs", [], |row| row
                    .get::<_, i64>(0))
                .expect("job count"),
            0
        );
        connection
            .execute_batch("DROP TRIGGER fail_learning_page")
            .expect("drop trigger");
        let dataset = materialize_dirty_roots(&mut connection, 11, 10)
            .expect("retry")
            .expect("dataset");
        let row: (String, i64) = connection
            .query_row(
                "SELECT features_json,label_revision FROM rr_examples WHERE dataset_id=?1",
                [dataset],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("example");
        assert_eq!(row, ("{\"quality\":null}".into(), 1));
    }

    #[test]
    fn rr_30_feature_snapshot_immutable_and_rr_31_explicit_feedback_next_day() {
        let mut connection = Connection::open_in_memory().expect("db");
        connection.execute_batch("PRAGMA foreign_keys=ON;CREATE TABLE conversations(id TEXT PRIMARY KEY);CREATE TABLE runtime_runs(id TEXT PRIMARY KEY);CREATE TABLE conversation_messages(id TEXT PRIMARY KEY,conversation_id TEXT,role TEXT,content TEXT,created_at TEXT);INSERT INTO conversations VALUES('c');INSERT INTO conversation_messages VALUES('answer','c','assistant','answer','1');").expect("base");
        crate::role_routing::schema::migrate(&connection).expect("routing");
        super::super::schema::migrate(&connection).expect("learning");
        connection
            .execute(
                "INSERT INTO rr_policy_versions VALUES('p',1,'{}','d',1)",
                [],
            )
            .expect("policy");
        connection.execute("INSERT INTO rr_roots(root_id,conversation_id,policy_id,phase,origin,presentation_mode,started_at_ms,scope_digest,result_message_id) VALUES('r','c','p','completed','text','visual',1,'','answer')", []).expect("root");
        connection.execute("INSERT INTO rr_decisions VALUES('d','r',0,NULL,'{\"quality\":null,\"complexity\":2}','[]','qwen','respond','[]','rules-v1','p',1)", []).expect("decision");
        connection
            .execute(
                "INSERT INTO rr_events VALUES('r',1,'answer_committed','{}',1)",
                [],
            )
            .expect("event");

        let mut datasets = Vec::new();
        for revision in 0..3 {
            if revision > 0 {
                let source = format!("source-{revision}");
                let kind = if revision == 1 {
                    "explicit_positive"
                } else {
                    "explicit_negative"
                };
                connection
                    .execute(
                        "INSERT INTO conversation_messages VALUES(?1,'c','user','feedback',?2)",
                        params![source, (revision + 1).to_string()],
                    )
                    .expect("source");
                connection.execute("INSERT INTO rr_feedback(id,target_answer_id,target_root_id,source_message_id,kind,evidence_json,label_source,confidence,extractor_version,status,created_at_ms) VALUES(?1,'answer','r',?2,?3,'{}','host',1.0,'host-v1','recorded',?4)", params![format!("feedback-{revision}"), source, kind, revision + 1]).expect("feedback");
            }
            mark_root_dirty(&connection, "r").expect("dirty");
            datasets.push(
                materialize_dirty_roots(&mut connection, 10 + revision, 10)
                    .expect("run")
                    .expect("dataset"),
            );
        }
        let rows = datasets
            .iter()
            .map(|dataset| connection.query_row("SELECT features_json,json_extract(labels_json,'$.outcome'),label_revision,eligible FROM rr_examples WHERE dataset_id=?1", [dataset], |row| Ok((row.get::<_,String>(0)?,row.get::<_,String>(1)?,row.get::<_,i64>(2)?,row.get::<_,i64>(3)?))).expect("example"))
            .collect::<Vec<_>>();
        assert!(rows
            .iter()
            .all(|row| row.0 == "{\"complexity\":2,\"quality\":null}"));
        assert_eq!(rows[0].1, "unknown");
        assert_eq!(rows[1].1, "positive");
        assert_eq!(
            (rows[2].1.as_str(), rows[2].2, rows[2].3),
            ("unknown", 2, 0)
        );
    }

    #[test]
    fn rr_35_artifact_is_shadow_only_and_requires_a_ready_dataset() {
        let c = Connection::open_in_memory().expect("db");
        c.execute_batch("CREATE TABLE rr_roots(root_id TEXT PRIMARY KEY);CREATE TABLE rr_decisions(id TEXT PRIMARY KEY);").expect("base");
        super::super::schema::migrate(&c).expect("schema");
        assert!(publish_shadow_artifact(&c, "missing", "candidates", "{}", "{}", 1).is_err());
        c.execute("INSERT INTO rr_datasets(id,upper_seq,feature_version,labeler_version,manifest_json,state,created_at_ms) VALUES('d',0,'f','l','{}','ready',1)",[]).expect("dataset");
        assert!(publish_shadow_artifact(&c, "d", "candidates", "{}", "{}", 2).is_err());
        for index in 0..20 {
            let decision_id = format!("decision-{index}");
            let example_id = format!("example-{index}");
            c.execute("INSERT INTO rr_decisions VALUES(?1)", [&decision_id])
                .expect("decision");
            c.execute("INSERT INTO rr_examples(id,dataset_id,decision_id,label_revision,feature_version,labeler_version,features_json,labels_json,eligible,created_at_ms) VALUES(?1,'d',?2,1,'f','l','{}','{}',1,1)", params![example_id,decision_id]).expect("example");
        }
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

    #[test]
    fn rr_35_small_sample_rules() {
        let connection = Connection::open_in_memory().expect("db");
        connection.execute_batch("CREATE TABLE rr_roots(root_id TEXT PRIMARY KEY);CREATE TABLE rr_decisions(id TEXT PRIMARY KEY);INSERT INTO rr_decisions VALUES('d');").expect("base");
        super::super::schema::migrate(&connection).expect("schema");
        connection.execute("INSERT INTO rr_datasets(id,upper_seq,feature_version,labeler_version,manifest_json,state,created_at_ms) VALUES('dataset',0,'f','l','{}','ready',1)", []).expect("dataset");
        connection.execute("INSERT INTO rr_examples(id,dataset_id,decision_id,label_revision,feature_version,labeler_version,features_json,labels_json,eligible,created_at_ms) VALUES('e','dataset','d',1,'f','l','{}','{}',1,1)", []).expect("example");
        assert_eq!(
            publish_shadow_artifact(&connection, "dataset", "recipes", "{}", "{}", 1).unwrap_err(),
            "Learning dataset has insufficient eligible examples"
        );
        assert_eq!(
            connection
                .query_row("SELECT count(*) FROM rr_ranker_artifacts", [], |row| row
                    .get::<_, i64>(0))
                .expect("artifacts"),
            0
        );
    }
}
