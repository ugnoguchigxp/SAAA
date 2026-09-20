//! Forgetting a source fences every learning artifact derived from it before its message is gone.
use rusqlite::{params, Connection};

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
    Ok(ForgetOutcome {
        datasets,
        artifacts,
        examples,
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
}
