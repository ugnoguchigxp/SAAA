pub(crate) fn typescript_bindings() -> String {
    fn declaration<T: TS>() -> String {
        format!("export {}", T::decl(&Config::default()))
    }
    [
        declaration::<RoutingSnapshotInput>(),
        declaration::<RoutingEventReplayInput>(),
        declaration::<RoutingCancelInput>(),
        declaration::<RoutingProposalDecisionInput>(),
        declaration::<AdaptiveRollbackInput>(),
        declaration::<AdaptiveArtifactSnapshot>(),
        declaration::<RoutingLearningSnapshot>(),
        declaration::<RoutingRootSnapshot>(),
        declaration::<RoutingSnapshot>(),
        declaration::<RoutingProposalSnapshot>(),
        declaration::<RoutingEventRecord>(),
    ]
    .join("\n\n")
}
fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or(0)
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rr_14_snapshot_and_replay_preserve_queue_order() {
        let connection = Connection::open_in_memory().expect("connection");
        connection.execute_batch("PRAGMA foreign_keys=ON; CREATE TABLE conversations(id TEXT PRIMARY KEY); CREATE TABLE runtime_runs(id TEXT PRIMARY KEY); CREATE TABLE conversation_messages(id TEXT PRIMARY KEY); INSERT INTO conversations VALUES('c');").expect("base");
        crate::role_routing::schema::migrate(&connection).expect("schema");
        connection
            .execute(
                "INSERT INTO rr_policy_versions VALUES('p',1,'{}','d',1)",
                [],
            )
            .expect("policy");
        connection.execute_batch("INSERT INTO rr_roots(root_id,conversation_id,policy_id,phase,origin,presentation_mode,started_at_ms,scope_digest) VALUES('active','c','p','responding','text','visual',1,''); INSERT INTO rr_roots(root_id,conversation_id,policy_id,phase,origin,presentation_mode,started_at_ms,scope_digest) VALUES('later','c','p','queued','text','visual',3,''); INSERT INTO rr_roots(root_id,conversation_id,policy_id,phase,origin,presentation_mode,started_at_ms,scope_digest) VALUES('first','c','p','queued','text','visual',2,''); INSERT INTO rr_roots(root_id,conversation_id,policy_id,phase,origin,presentation_mode,started_at_ms,scope_digest) VALUES('completed','c','p','completed','text','visual',4,''); INSERT INTO rr_events VALUES('active',1,'root_started','{}',1); INSERT INTO rr_events VALUES('active',2,'activity','{}',2); INSERT INTO rr_events VALUES('completed',1,'answer_committed','{}',4);").expect("rows");
        let projection = snapshot(&connection, "c").expect("snapshot");
        assert_eq!(
            projection.active.as_ref().expect("active").root_id,
            "active"
        );
        assert_eq!(
            projection
                .queued
                .iter()
                .map(|root| root.root_id.as_str())
                .collect::<Vec<_>>(),
            vec!["first", "later"]
        );
        assert_eq!(
            replay(&connection, "active", 1).expect("replay")[0].kind,
            "activity"
        );
        assert_eq!(projection.recent_root_ids[0], "completed");
        assert_eq!(
            replay(&connection, "completed", 0).expect("terminal replay")[0].kind,
            "answer_committed"
        );
    }

    #[test]
    fn rr_27_cancel_is_durable_and_replayable() {
        let mut connection = Connection::open_in_memory().expect("connection");
        connection.execute_batch("PRAGMA foreign_keys=ON; CREATE TABLE conversations(id TEXT PRIMARY KEY); CREATE TABLE runtime_runs(id TEXT PRIMARY KEY); CREATE TABLE conversation_messages(id TEXT PRIMARY KEY); INSERT INTO conversations VALUES('c');").expect("base");
        crate::role_routing::schema::migrate(&connection).expect("schema");
        connection
            .execute(
                "INSERT INTO rr_policy_versions VALUES('p',1,'{}','d',1)",
                [],
            )
            .expect("policy");
        connection.execute_batch("INSERT INTO rr_roots(root_id,conversation_id,policy_id,phase,origin,presentation_mode,started_at_ms,scope_digest) VALUES('r','c','p','responding','text','visual',1,''); INSERT INTO rr_events VALUES('r',1,'root_started','{}',1);").expect("root");
        crate::role_routing::coordinator::apply(
            &mut connection,
            "r",
            crate::role_routing::reducer::Event::Cancel,
            2,
        )
        .expect("cancel");
        let root = root_snapshot(&connection, "r").expect("snapshot");
        assert_eq!(root.phase, "cancelled");
        assert!(root.cancel_requested);
        assert_eq!(root.last_event_seq, 2);
        assert_eq!(
            replay(&connection, "r", 1).expect("events")[0].kind,
            "root_cancelled"
        );
    }

    #[test]
    fn rr_26_proposal_snapshot_survives_reload_without_losing_consumption() {
        let connection = Connection::open_in_memory().expect("connection");
        connection.execute_batch("PRAGMA foreign_keys=ON; CREATE TABLE conversations(id TEXT PRIMARY KEY); CREATE TABLE runtime_runs(id TEXT PRIMARY KEY); CREATE TABLE conversation_messages(id TEXT PRIMARY KEY); INSERT INTO conversations VALUES('c');").expect("base");
        crate::role_routing::schema::migrate(&connection).expect("schema");
        connection
            .execute(
                "INSERT INTO rr_policy_versions VALUES('p',1,'{}','d',1)",
                [],
            )
            .expect("policy");
        connection.execute_batch("INSERT INTO rr_roots(root_id,conversation_id,policy_id,revision,phase,origin,presentation_mode,started_at_ms,scope_digest) VALUES('r','c','p',0,'responding','text','visual',1,''); INSERT INTO rr_premium_proposals(id,root_id,candidate_id,policy_id,revision,estimated_cost_micros,expires_at_ms,status,created_at_ms,approved_at_ms,consumed_at_ms) VALUES('offer','r','astra','p',0,NULL,999,'approved',2,3,4);").expect("proposal");

        for _reload in 0..2 {
            let projection = snapshot(&connection, "c").expect("snapshot");
            assert_eq!(projection.proposals.len(), 1);
            assert_eq!(projection.proposals[0].status, "approved");
            assert!(projection.proposals[0].consumed);
            assert_eq!(projection.proposals[0].estimated_cost_micros, None);
        }
    }

    #[test]
    fn rr_37_learning_snapshot_contains_counts_only() {
        let connection = Connection::open_in_memory().expect("connection");
        connection
            .execute_batch(
                "CREATE TABLE rr_roots(root_id TEXT PRIMARY KEY);
                 CREATE TABLE rr_decisions(id TEXT PRIMARY KEY);",
            )
            .expect("base");
        crate::role_routing::learning::schema::migrate(&connection).expect("learning schema");
        connection
            .execute_batch(
                "INSERT INTO rr_datasets(id,upper_seq,feature_version,labeler_version,manifest_json,state,created_at_ms) VALUES('ready',0,'f','l','{}','ready',1);
                 INSERT INTO rr_ranker_artifacts(id,dataset_id,algorithm,feature_version,candidate_fingerprint,weights_json,metrics_json,digest,state,created_at_ms) VALUES('artifact','ready','linear-v1','f','c','{}','{}','d','shadow',1);",
            )
            .expect("learning rows");
        let snapshot = learning_snapshot(&connection).expect("snapshot");
        assert_eq!(snapshot.dirty_roots, 0);
        assert_eq!(snapshot.ready_datasets, 1);
        assert_eq!(snapshot.active_artifacts, 1);
        assert!(snapshot.adaptive_artifacts.is_empty());
    }

    #[test]
    fn ai_13_learning_snapshot_explains_pending_adaptive_artifact() {
        let connection = Connection::open_in_memory().expect("connection");
        connection
            .execute_batch(
                "CREATE TABLE rr_roots(root_id TEXT PRIMARY KEY);
                 CREATE TABLE rr_decisions(id TEXT PRIMARY KEY);",
            )
            .expect("base");
        crate::role_routing::learning::schema::migrate(&connection).expect("learning schema");
        crate::adaptive_improvement::migrate(&connection).expect("adaptive schema");
        let candidates = vec!["a".to_string(), "b".to_string()];
        let artifact = crate::adaptive_improvement::create_artifact(
            &connection,
            crate::adaptive_improvement::Domain::Tool,
            "project-a",
            &candidates,
            r#"{"a":0.75,"b":0.25}"#,
            0,
            1,
        )
        .expect("artifact");
        let snapshot = learning_snapshot(&connection).expect("snapshot");
        assert_eq!(snapshot.adaptive_artifacts.len(), 1);
        let row = &snapshot.adaptive_artifacts[0];
        assert_eq!(row.id, artifact);
        assert_eq!(row.domain, "tool");
        assert_eq!(row.eligible_examples, 0);
        assert_eq!(row.best_observed_score, Some(0.75));
        assert!(row.reason.contains("次の選択にはまだ使われません"));
    }
}
