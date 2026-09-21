//! Forgetting a source fences every learning artifact derived from it before its message is gone.
use rusqlite::{params, Connection};
use std::{fs, path::Path};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ForgetOutcome {
    pub(crate) datasets: usize,
    pub(crate) artifacts: usize,
    pub(crate) examples: usize,
}

/// Invalidates data derived from a user message. The caller may subsequently delete that message
/// in the same writer transaction. Inputs are linked through their routing root; feedback is
/// linked through its immutable feedback receipt. We delete affected examples as well so an
/// invalidated dataset cannot be accidentally reconstructed from a stale row.
pub(crate) fn forget_source(
    connection: &Connection,
    source_message_id: &str,
) -> Result<ForgetOutcome, String> {
    let matching_examples = "SELECT DISTINCT e.id FROM rr_examples e
        JOIN rr_example_sources s ON s.example_id=e.id
        WHERE s.scope_key IN (SELECT root_id FROM rr_inputs WHERE message_id=?1)
           OR (s.source_kind='feedback' AND s.source_id IN
               (SELECT id FROM rr_feedback WHERE source_message_id=?1))";
    let matching_datasets =
        format!("SELECT DISTINCT dataset_id FROM rr_examples WHERE id IN ({matching_examples})");
    let dataset_ids = connection
        .prepare(&matching_datasets)
        .map_err(|error| error.to_string())?
        .query_map([source_message_id], |row| row.get::<_, String>(0))
        .map_err(|error| error.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string())?;
    let artifacts = connection
        .execute(
            &format!(
                "UPDATE rr_ranker_artifacts SET state='invalidated'
                 WHERE state IN ('candidate','shadow') AND dataset_id IN ({matching_datasets})"
            ),
            [source_message_id],
        )
        .map_err(|error| error.to_string())?;
    let datasets = connection
        .execute(
            &format!(
                "UPDATE rr_datasets SET state='invalidated'
                 WHERE state='ready' AND id IN ({matching_datasets})"
            ),
            [source_message_id],
        )
        .map_err(|error| error.to_string())?;
    let examples = connection
        .execute(
            &format!("DELETE FROM rr_examples WHERE id IN ({matching_examples})"),
            params![source_message_id],
        )
        .map_err(|error| error.to_string())?;
    for dataset_id in dataset_ids {
        connection.execute(
            "INSERT INTO rr_cleanup_journal(dataset_id,state,attempts,updated_at_ms) VALUES(?1,'pending',0,0)
             ON CONFLICT(dataset_id) DO UPDATE SET state='pending',last_error_code=NULL",
            [dataset_id],
        ).map_err(|error| error.to_string())?;
    }
    Ok(ForgetOutcome {
        datasets,
        artifacts,
        examples,
    })
}

/// Retries deletion of the fixed export filenames for invalidated datasets. The journal stores
/// dataset ids and closed error codes only; local paths and file contents never enter SQLite.
pub(crate) fn cleanup_invalidated_exports(
    connection: &Connection,
    directory: &Path,
    now_ms: i64,
) -> Result<usize, String> {
    let pending = connection
        .prepare(
            "SELECT dataset_id FROM rr_cleanup_journal WHERE state='pending' ORDER BY dataset_id",
        )
        .map_err(|error| error.to_string())?
        .query_map([], |row| row.get::<_, String>(0))
        .map_err(|error| error.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string())?;
    let mut completed = 0;
    for dataset_id in pending {
        if crate::validate_identifier(&dataset_id, "routing dataset id").is_err() {
            connection.execute("UPDATE rr_cleanup_journal SET attempts=attempts+1,last_error_code='invalid_dataset_id',updated_at_ms=?1 WHERE dataset_id=?2", params![now_ms,dataset_id]).map_err(|error| error.to_string())?;
            continue;
        }
        let paths = [
            directory.join(format!("{dataset_id}.jsonl")),
            directory.join(format!("{dataset_id}.manifest.json")),
            directory.join(format!("{dataset_id}.jsonl.tmp")),
            directory.join(format!("{dataset_id}.manifest.json.tmp")),
        ];
        let failed = paths.iter().any(|path| match fs::remove_file(path) {
            Ok(()) => false,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
            Err(_) => true,
        });
        if failed {
            connection.execute("UPDATE rr_cleanup_journal SET attempts=attempts+1,last_error_code='filesystem_cleanup_failed',updated_at_ms=?1 WHERE dataset_id=?2", params![now_ms,dataset_id]).map_err(|error| error.to_string())?;
        } else {
            connection.execute("UPDATE rr_cleanup_journal SET state='completed',attempts=attempts+1,last_error_code=NULL,updated_at_ms=?1 WHERE dataset_id=?2", params![now_ms,dataset_id]).map_err(|error| error.to_string())?;
            completed += 1;
        }
    }
    Ok(completed)
}

/// Production cleanup keeps filesystem I/O outside the process-wide SQLite writer lock. The
/// pending list and completion receipts are short database operations on either side of deletion.
pub(crate) fn cleanup_invalidated_exports_with_writer(
    writer: &crate::persistence::SqliteWriter,
    directory: &Path,
    now_ms: i64,
) -> Result<usize, String> {
    let pending = writer.read_serialized(|connection| {
        connection
            .prepare(
                "SELECT dataset_id FROM rr_cleanup_journal WHERE state='pending' ORDER BY dataset_id",
            )
            .map_err(|error| error.to_string())?
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(|error| error.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| error.to_string())
    })?;
    let outcomes = pending
        .into_iter()
        .map(|dataset_id| {
            let error_code =
                if crate::validate_identifier(&dataset_id, "routing dataset id").is_err() {
                    Some("invalid_dataset_id")
                } else {
                    let failed = [
                        directory.join(format!("{dataset_id}.jsonl")),
                        directory.join(format!("{dataset_id}.manifest.json")),
                        directory.join(format!("{dataset_id}.jsonl.tmp")),
                        directory.join(format!("{dataset_id}.manifest.json.tmp")),
                    ]
                    .iter()
                    .any(|path| match fs::remove_file(path) {
                        Ok(()) => false,
                        Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
                        Err(_) => true,
                    });
                    failed.then_some("filesystem_cleanup_failed")
                };
            (dataset_id, error_code)
        })
        .collect::<Vec<_>>();
    writer.write(|connection| {
        let mut completed = 0;
        for (dataset_id, error_code) in &outcomes {
            if let Some(error_code) = error_code {
                connection.execute("UPDATE rr_cleanup_journal SET attempts=attempts+1,last_error_code=?1,updated_at_ms=?2 WHERE dataset_id=?3 AND state='pending'", params![error_code,now_ms,dataset_id]).map_err(|error| error.to_string())?;
            } else {
                completed += connection.execute("UPDATE rr_cleanup_journal SET state='completed',attempts=attempts+1,last_error_code=NULL,updated_at_ms=?1 WHERE dataset_id=?2 AND state='pending'", params![now_ms,dataset_id]).map_err(|error| error.to_string())?;
            }
        }
        Ok(completed)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rr_37_forget_invalidates_before_dispatch() {
        let connection = Connection::open_in_memory().expect("database");
        connection
            .execute_batch(
                "PRAGMA foreign_keys=ON;
                 CREATE TABLE conversations(id TEXT PRIMARY KEY);
                 CREATE TABLE runtime_runs(id TEXT PRIMARY KEY);
                 CREATE TABLE conversation_messages(id TEXT PRIMARY KEY);
                 INSERT INTO conversations VALUES('c');",
            )
            .expect("base schema");
        crate::role_routing::schema::migrate(&connection).expect("routing schema");
        super::super::schema::migrate(&connection).expect("learning schema");
        connection
            .execute_batch(
                "INSERT INTO rr_policy_versions VALUES('p',1,'{}','d',1);
                 INSERT INTO rr_roots(root_id,conversation_id,policy_id,phase,origin,presentation_mode,started_at_ms,scope_digest) VALUES('r','c','p','completed','text','visual',1,'');
                 INSERT INTO conversation_messages VALUES('input');
                 INSERT INTO rr_inputs(input_id,root_id,conversation_id,message_id,payload_digest,origin,disposition,received_at_ms) VALUES('i','r','c','input','d','text','started',1);
                 INSERT INTO rr_decisions(id,root_id,revision,features_json,candidates_json,action,reason_codes_json,ranker_version,policy_id,created_at_ms) VALUES('decision','r',0,'{}','[]','respond','[]','rules','p',1);
                 INSERT INTO rr_datasets(id,upper_seq,feature_version,labeler_version,manifest_json,state,created_at_ms) VALUES('dataset',1,'rr-features-v1','labeler','{}','ready',1);
                 INSERT INTO rr_examples(id,dataset_id,decision_id,label_revision,feature_version,labeler_version,features_json,labels_json,eligible,created_at_ms) VALUES('example','dataset','decision',1,'rr-features-v1','labeler','{}','{}',1,1);
                 INSERT INTO rr_example_sources(example_id,source_kind,source_id,source_version,scope_key) VALUES('example','decision','decision','1','r');
                 INSERT INTO rr_ranker_artifacts(id,dataset_id,algorithm,feature_version,candidate_fingerprint,weights_json,metrics_json,digest,state,created_at_ms) VALUES('artifact','dataset','linear-v1','rr-features-v1','candidates','{}','{}','digest','shadow',1);",
            )
            .expect("fixture");

        let outcome = forget_source(&connection, "input").expect("forget");
        assert_eq!(
            outcome,
            ForgetOutcome {
                datasets: 1,
                artifacts: 1,
                examples: 1
            }
        );
        assert_eq!(
            connection
                .query_row(
                    "SELECT state FROM rr_ranker_artifacts WHERE id='artifact'",
                    [],
                    |row| row.get::<_, String>(0)
                )
                .expect("artifact state"),
            "invalidated"
        );
        assert_eq!(
            connection
                .query_row(
                    "SELECT state FROM rr_datasets WHERE id='dataset'",
                    [],
                    |row| row.get::<_, String>(0)
                )
                .expect("dataset state"),
            "invalidated"
        );
        assert_eq!(
            connection
                .query_row("SELECT count(*) FROM rr_examples", [], |row| row
                    .get::<_, i64>(0))
                .expect("examples"),
            0
        );
    }

    #[test]
    fn rr_37_forget_feedback_source_invalidates_its_target_dataset() {
        let connection = Connection::open_in_memory().expect("database");
        connection
            .execute_batch(
                "PRAGMA foreign_keys=ON;
                 CREATE TABLE conversations(id TEXT PRIMARY KEY);
                 CREATE TABLE runtime_runs(id TEXT PRIMARY KEY);
                 CREATE TABLE conversation_messages(id TEXT PRIMARY KEY);
                 INSERT INTO conversations VALUES('c');",
            )
            .expect("base schema");
        crate::role_routing::schema::migrate(&connection).expect("routing schema");
        super::super::schema::migrate(&connection).expect("learning schema");
        connection
            .execute_batch(
                "INSERT INTO rr_policy_versions VALUES('p',1,'{}','d',1);
                 INSERT INTO rr_roots(root_id,conversation_id,policy_id,phase,origin,presentation_mode,started_at_ms,scope_digest) VALUES('r','c','p','completed','text','visual',1,'');
                 INSERT INTO conversation_messages VALUES('answer');
                 INSERT INTO conversation_messages VALUES('feedback');
                 INSERT INTO rr_datasets(id,upper_seq,feature_version,labeler_version,manifest_json,state,created_at_ms) VALUES('dataset',1,'rr-features-v1','labeler','{}','ready',1);
                 INSERT INTO rr_decisions(id,root_id,revision,features_json,candidates_json,action,reason_codes_json,ranker_version,policy_id,created_at_ms) VALUES('decision','r',0,'{}','[]','respond','[]','rules','p',1);
                 INSERT INTO rr_examples(id,dataset_id,decision_id,label_revision,feature_version,labeler_version,features_json,labels_json,eligible,created_at_ms) VALUES('example','dataset','decision',1,'rr-features-v1','labeler','{}','{}',1,1);
                 INSERT INTO rr_feedback(id,target_answer_id,target_root_id,source_message_id,kind,evidence_json,label_source,extractor_version,status,created_at_ms) VALUES('receipt','answer','r','feedback','explicit_negative','{}','host','host-v1','recorded',1);
                 INSERT INTO rr_example_sources(example_id,source_kind,source_id,source_version,scope_key) VALUES('example','feedback','receipt','host-v1','r');
                 INSERT INTO rr_ranker_artifacts(id,dataset_id,algorithm,feature_version,candidate_fingerprint,weights_json,metrics_json,digest,state,created_at_ms) VALUES('artifact','dataset','linear-v1','rr-features-v1','candidates','{}','{}','digest','shadow',1);",
            )
            .expect("fixture");

        assert_eq!(
            forget_source(&connection, "feedback")
                .expect("forget")
                .artifacts,
            1
        );
        assert_eq!(
            connection
                .query_row(
                    "SELECT state FROM rr_ranker_artifacts WHERE id='artifact'",
                    [],
                    |row| row.get::<_, String>(0)
                )
                .expect("artifact state"),
            "invalidated"
        );
    }

    #[test]
    fn rr_37_cleanup_retry_keeps_artifact_invalidated() {
        let connection = Connection::open_in_memory().expect("database");
        connection.execute_batch("CREATE TABLE rr_roots(root_id TEXT PRIMARY KEY); CREATE TABLE rr_decisions(id TEXT PRIMARY KEY); INSERT INTO rr_roots VALUES('r'); INSERT INTO rr_decisions VALUES('d');").expect("base");
        super::super::schema::migrate(&connection).expect("learning schema");
        connection.execute("INSERT INTO rr_datasets(id,upper_seq,feature_version,labeler_version,manifest_json,state,created_at_ms) VALUES('dataset',1,'f','l','{}','invalidated',1)", []).expect("dataset");
        connection.execute("INSERT INTO rr_cleanup_journal(dataset_id,state,updated_at_ms) VALUES('dataset','pending',1)", []).expect("journal");
        let temporary = tempfile::tempdir().expect("temporary");
        let blocked = temporary.path().join("blocked");
        fs::write(&blocked, b"not a directory").expect("blocker");
        assert_eq!(
            cleanup_invalidated_exports(&connection, &blocked, 2).expect("retry"),
            0
        );
        assert_eq!(connection.query_row("SELECT state||':'||attempts||':'||last_error_code FROM rr_cleanup_journal WHERE dataset_id='dataset'", [], |row| row.get::<_,String>(0)).expect("pending"), "pending:1:filesystem_cleanup_failed");
        fs::remove_file(&blocked).expect("remove blocker");
        fs::create_dir(&blocked).expect("directory");
        fs::write(blocked.join("dataset.jsonl"), b"fixture").expect("export");
        fs::write(blocked.join("dataset.manifest.json"), b"fixture").expect("manifest");
        assert_eq!(
            cleanup_invalidated_exports(&connection, &blocked, 3).expect("cleanup"),
            1
        );
        assert_eq!(connection.query_row("SELECT state||':'||attempts FROM rr_cleanup_journal WHERE dataset_id='dataset'", [], |row| row.get::<_,String>(0)).expect("completed"), "completed:2");
        assert!(!blocked.join("dataset.jsonl").exists());
        assert_eq!(
            connection
                .query_row(
                    "SELECT state FROM rr_datasets WHERE id='dataset'",
                    [],
                    |row| row.get::<_, String>(0)
                )
                .expect("dataset state"),
            "invalidated"
        );
    }
}
