#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const T0: &str = "1787961600000";
    const T1: &str = "1787961660000";
    const T2: &str = "1787961720000";

    fn database() -> Connection {
        let connection = Connection::open_in_memory().expect("database opens");
        connection
            .execute_batch(
                "PRAGMA foreign_keys=ON;
                 CREATE TABLE conversations (
                   id TEXT PRIMARY KEY,
                   title TEXT,
                   task_mode TEXT NOT NULL,
                   created_at TEXT NOT NULL,
                   updated_at TEXT NOT NULL
                 );
                 CREATE TABLE conversation_messages (
                   id TEXT PRIMARY KEY,
                   conversation_id TEXT NOT NULL,
                   role TEXT NOT NULL,
                   content TEXT NOT NULL,
                   created_at TEXT NOT NULL,
                   FOREIGN KEY(conversation_id) REFERENCES conversations(id) ON DELETE CASCADE
                 );
                 INSERT INTO conversations(id,title,task_mode,created_at,updated_at)
                 VALUES('primary','Primary','conversation','2026-08-29T00:00:00.000Z','2026-08-29T00:00:00.000Z');",
            )
            .expect("base schema creates");
        migrate_v11_to_v12(&connection).expect("memory schema migrates");
        ensure_continuity_state(&connection, "primary", T0).expect("continuity state initializes");
        connection
    }

    fn completed_source(connection: &mut Connection) -> SourceWindow {
        connection
            .execute_batch(
                "INSERT INTO conversation_messages(id,conversation_id,role,content,created_at)
                 VALUES
                   ('user-1','primary','user','Private project Alpha request','2026-08-29T00:00:10.000Z'),
                   ('assistant-1','primary','assistant','Private project Alpha response','2026-08-29T00:00:20.000Z');",
            )
            .expect("turn inserts");
        let transaction = connection.transaction().expect("transaction starts");
        let window =
            record_completed_turn(&transaction, "user-1", "assistant-1", T1).expect("turn records");
        transaction.commit().expect("turn commits");
        window
    }

    #[test]
    fn migration_is_idempotent_and_preserves_raw_conversation_rows() {
        let connection = database();
        connection
            .execute(
                "INSERT INTO conversation_messages(
                   id,conversation_id,role,content,created_at
                 ) VALUES('raw-kept','primary','user','Raw text remains unchanged',?1)",
                params![T0],
            )
            .expect("raw fixture inserts");
        let before: Vec<(String, String)> = connection
            .prepare("SELECT id,content FROM conversation_messages ORDER BY id")
            .expect("query prepares")
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .expect("query runs")
            .collect::<Result<_, _>>()
            .expect("rows load");

        migrate_v11_to_v12(&connection).expect("second migration succeeds");

        let after: Vec<(String, String)> = connection
            .prepare("SELECT id,content FROM conversation_messages ORDER BY id")
            .expect("query prepares")
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .expect("query runs")
            .collect::<Result<_, _>>()
            .expect("rows load");
        assert_eq!(before, after);
        let table_count: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name IN (
                   'continuity_state','memory_source_windows','continuity_capsule_revisions',
                   'user_profile_items','working_state_items','memory_reflection_jobs',
                   'memory_outbox','memory_decision_events','context_projection_events'
                 )",
                [],
                |row| row.get(0),
            )
            .expect("tables count");
        assert_eq!(table_count, 9);
    }

    #[test]
    fn completed_turn_is_idempotent_without_creating_unhandled_jobs() {
        let mut connection = database();
        let first = completed_source(&mut connection);
        let transaction = connection.transaction().expect("transaction starts");
        let second = record_completed_turn(&transaction, "user-1", "assistant-1", T2)
            .expect("duplicate turn is accepted");
        transaction.commit().expect("transaction commits");

        assert_eq!(first, second);
        assert!(!first.source_ref.contains("Alpha"));
        let source_count: i64 = connection
            .query_row("SELECT COUNT(*) FROM memory_source_windows", [], |row| {
                row.get(0)
            })
            .expect("source count");
        let job_count: i64 = connection
            .query_row("SELECT COUNT(*) FROM memory_reflection_jobs", [], |row| {
                row.get(0)
            })
            .expect("job count");
        assert_eq!((source_count, job_count), (1, 0));

        connection
            .execute_batch(
                "INSERT INTO conversation_messages(id,conversation_id,role,content,created_at)
                 VALUES
                   ('user-2','primary','user','Next request','1787961780000'),
                   ('assistant-2','primary','assistant','Next response','1787961840000');",
            )
            .expect("second turn inserts");
        let transaction = connection.transaction().expect("transaction starts");
        assert!(record_completed_turn(&transaction, "user-1", "assistant-2", T2).is_err());
    }

    #[test]
    fn only_confirmed_profile_items_project_and_source_deletion_tombstones_them() {
        let mut connection = database();
        let source = completed_source(&mut connection);
        let candidate = insert_profile_candidate(
            &connection,
            "communication",
            "response.style",
            &json!({"tone": "concise"}),
            10,
            &source.id,
            T1,
        )
        .expect("candidate inserts");
        let unavailable_candidate = insert_profile_candidate(
            &connection,
            "preference",
            "response.language",
            &json!({"language": "ja"}),
            5,
            &source.id,
            T1,
        )
        .expect("second candidate inserts");
        assert!(load_projection_items(&connection, T1)
            .expect("projection loads")
            .is_empty());

        confirm_profile_candidate(&mut connection, &candidate, T2).expect("candidate confirms");
        put_working_state(
            &mut connection,
            WorkingStateInput {
                item_kind: "constraint",
                semantic_key: "response.style",
                value: &json!({"tone": "temporary"}),
                priority: 100,
                source_window_id: &source.id,
                valid_until: None,
            },
            T2,
        )
        .expect("overlapping working state inserts");
        let projected = load_projection_items(&connection, T2).expect("projection loads");
        assert_eq!(projected.len(), 1);
        assert_eq!(projected[0].memory_class, "user_core");

        connection
            .execute("DELETE FROM conversation_messages WHERE id='user-1'", [])
            .expect("source message deletes");
        let availability: (String, Option<String>, Option<String>) = connection
            .query_row(
                "SELECT availability,start_message_id,end_message_id
                 FROM memory_source_windows WHERE id=?1",
                params![source.id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .expect("source tombstone loads");
        assert_eq!(availability, ("deleted".into(), None, None));
        assert!(load_projection_items(&connection, T2)
            .expect("projection loads")
            .is_empty());
        assert!(confirm_profile_candidate(&mut connection, &unavailable_candidate, T2).is_err());
    }

    #[test]
    fn working_state_honors_ttl_and_resolution() {
        let mut connection = database();
        let source = completed_source(&mut connection);
        put_working_state(
            &mut connection,
            WorkingStateInput {
                item_kind: "open_loop",
                semantic_key: "followup.pending",
                value: &json!({"summary": "follow up"}),
                priority: 20,
                source_window_id: &source.id,
                valid_until: Some(T2),
            },
            T1,
        )
        .expect("working item inserts");
        assert_eq!(
            load_projection_items(&connection, T1)
                .expect("projection loads")
                .len(),
            1
        );
        assert!(load_projection_items(&connection, T2)
            .expect("projection loads")
            .is_empty());
        assert_eq!(
            expire_working_state(&connection, T2).expect("item expires"),
            1
        );

        put_working_state(
            &mut connection,
            WorkingStateInput {
                item_kind: "commitment",
                semantic_key: "followup.pending",
                value: &json!({"summary": "new follow up"}),
                priority: 20,
                source_window_id: &source.id,
                valid_until: None,
            },
            T2,
        )
        .expect("replacement inserts");
        assert_eq!(
            resolve_working_state(&connection, "followup.pending", T2).expect("item resolves"),
            1
        );
        assert!(load_projection_items(&connection, T2)
            .expect("projection loads")
            .is_empty());
    }

    #[test]
    fn capsule_activation_is_atomic_and_keeps_one_active_revision() {
        let mut connection = database();
        let source = completed_source(&mut connection);
        let first = [CapsuleItemInput {
            item_kind: "active_referent".into(),
            semantic_key: "subject.current".into(),
            value_json: json!({"subject": "abstract client"}),
            priority: 10,
            source_window_id: source.id.clone(),
            valid_until: None,
        }];
        assert_eq!(
            activate_capsule_revision(
                &mut connection,
                "2026-08-29T00:00:20.000Z",
                "assistant-1",
                &"a".repeat(64),
                &first,
                T1,
            )
            .expect("first capsule activates"),
            1
        );
        let invalid = [CapsuleItemInput {
            item_kind: "invalid".into(),
            semantic_key: "subject.invalid".into(),
            value_json: json!(true),
            priority: 0,
            source_window_id: source.id.clone(),
            valid_until: None,
        }];
        assert!(activate_capsule_revision(
            &mut connection,
            "2026-08-29T00:00:20.000Z",
            "assistant-1",
            &"b".repeat(64),
            &invalid,
            T2,
        )
        .is_err());
        let statuses: (i64, i64) = connection
            .query_row(
                "SELECT COUNT(*) FILTER (WHERE status='active'),
                        COUNT(*) FILTER (WHERE status='building')
                 FROM continuity_capsule_revisions",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("statuses load");
        assert_eq!(statuses, (1, 0));
        assert_eq!(
            load_projection_items(&connection, T2)
                .expect("projection loads")
                .iter()
                .filter(|item| item.memory_class == "continuity_capsule")
                .count(),
            1
        );
    }

    #[test]
    fn startup_cancels_jobs_that_have_no_worker() {
        let connection = database();
        connection.execute("INSERT INTO memory_reflection_jobs(id,job_kind,status,created_at,updated_at) VALUES('queued','capsule_refresh','queued',?1,?1),('running','capsule_refresh','running',?1,?1)", [T1]).expect("jobs insert");
        assert_eq!(
            cancel_unhandled_jobs(&connection, T2).expect("jobs cancel"),
            2
        );
        let cancelled: i64 = connection.query_row("SELECT COUNT(*) FROM memory_reflection_jobs WHERE status='cancelled' AND result_code='worker-unavailable'", [], |row| row.get(0)).expect("cancelled jobs count");
        assert_eq!(cancelled, 2);
    }
}
