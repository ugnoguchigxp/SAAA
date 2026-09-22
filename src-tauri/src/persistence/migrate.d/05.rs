#[test]
    fn version_eleven_database_is_backed_up_before_memory_v12() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("v11.sqlite3");
        let connection = Connection::open(&path).expect("database opens");
        initialize_database(&connection).expect("schema initializes");
        connection
            .execute(
                "INSERT INTO conversations(id,title,task_mode,created_at,updated_at)
             VALUES('v11-kept','Keep','coding','1','1')",
                [],
            )
            .expect("fixture inserts");
        connection
            .pragma_update(None, "user_version", 11)
            .expect("v11 fixture");
        drop(connection);

        let connection = Connection::open(&path).expect("database reopens");
        let backup = backup_before_migration(&connection, &path)
            .expect("backup succeeds")
            .expect("v11 backup is created");
        initialize_database(&connection).expect("v12 migration succeeds");

        let migrated_version: i64 = connection
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .expect("migrated version reads");
        let backup_connection = Connection::open(backup).expect("backup reopens");
        let backup_version: i64 = backup_connection
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .expect("backup version reads");
        let title: String = backup_connection
            .query_row(
                "SELECT title FROM conversations WHERE id='v11-kept'",
                [],
                |row| row.get(0),
            )
            .expect("backup data remains");
        assert_eq!(
            migrated_version,
            crate::persistence::schema::DATABASE_SCHEMA_VERSION
        );
        assert_eq!(backup_version, 11);
        assert_eq!(title, "Keep");
    }
#[test]
    fn settings_v9_is_backed_up_before_v10_shape_migration() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("settings-v9.sqlite3");
        let connection = Connection::open(&path).expect("database opens");
        initialize_database(&connection).expect("schema initializes");
        connection
            .execute(
                "UPDATE settings_documents
                 SET schema_version=9,
                     value_json=CASE
                       WHEN namespace='providers.model' THEN json_remove(value_json, '$.maxOutputTokens')
                       WHEN namespace='voice.runtime' THEN json_remove(value_json, '$.sttHost')
                       ELSE value_json
                     END",
                [],
            )
            .expect("v9 settings fixture writes");
        drop(connection);

        let connection = Connection::open(&path).expect("database reopens");
        let backup = backup_before_migration(&connection, &path)
            .expect("backup succeeds")
            .expect("settings backup is created");
        initialize_database(&connection).expect("settings v10 migration succeeds");

        let backup_connection = Connection::open(backup).expect("backup reopens");
        let (backup_schema, backup_tokens): (i64, Option<i64>) = backup_connection
            .query_row(
                "SELECT schema_version, json_extract(value_json, '$.maxOutputTokens')
                 FROM settings_documents
                 WHERE namespace='providers.model' AND key='default'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("v9 backup remains readable");
        assert_eq!(backup_schema, 9);
        assert!(backup_tokens.is_none());

        let migrated = list_settings_documents(&connection).expect("v10 settings load");
        assert!(migrated
            .iter()
            .all(|document| document.schema_version == SETTINGS_SCHEMA_VERSION));
    }
#[test]
    fn current_database_with_a_missing_settings_table_is_backed_up_before_repair() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("missing-settings.sqlite3");
        let connection = Connection::open(&path).expect("database opens");
        initialize_database(&connection).expect("schema initializes");
        connection
            .execute(
                "INSERT INTO conversations(id,title,task_mode,created_at,updated_at)
                 VALUES('repair-kept','Keep','coding','1','1')",
                [],
            )
            .expect("user data inserts");
        connection
            .execute("DROP TABLE settings_documents", [])
            .expect("settings corruption fixture writes");
        drop(connection);

        let connection = Connection::open(&path).expect("database reopens");
        let backup = backup_before_migration(&connection, &path)
            .expect("backup succeeds")
            .expect("repair backup is created");
        let backup_connection = Connection::open(backup).expect("backup reopens");
        let title: String = backup_connection
            .query_row(
                "SELECT title FROM conversations WHERE id='repair-kept'",
                [],
                |row| row.get(0),
            )
            .expect("user data remains in backup");
        assert_eq!(title, "Keep");
    }
#[test]
    fn version_six_runtime_rows_gain_nullable_supervisor_columns() {
        let connection = Connection::open_in_memory().expect("database opens");
        initialize_database(&connection).expect("schema initializes");
        connection
            .execute_batch(
                "ALTER TABLE runtime_runs DROP COLUMN failure_code;
             ALTER TABLE runtime_runs DROP COLUMN supervisor_version;
             ALTER TABLE runtime_runs DROP COLUMN last_progress_at;",
            )
            .expect("v6 columns remove");
        connection
            .pragma_update(None, "user_version", 6)
            .expect("v6 fixture");
        initialize_database(&connection).expect("v8 migration succeeds");
        for column in ["failure_code", "supervisor_version", "last_progress_at"] {
            let exists: bool = connection
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM pragma_table_info('runtime_runs') WHERE name=?1)",
                    [column],
                    |row| row.get(0),
                )
                .expect("column check succeeds");
            assert!(exists, "missing column: {column}");
        }
        connection
            .execute(
                "INSERT INTO conversations(id,task_mode,created_at,updated_at)
             VALUES('nullable','coding','1','1')",
                [],
            )
            .expect("conversation inserts");
        connection
            .execute(
                "INSERT INTO runtime_runs(id,conversation_id,route_kind,status,started_at)
             VALUES('nullable-run','nullable','coding.assist','running','1')",
                [],
            )
            .expect("nullable migrated columns accept old rows");
    }
#[test]
    fn startup_drops_meeting_product_tables() {
        let connection = Connection::open_in_memory().expect("database opens");
        initialize_database(&connection).expect("schema initializes");
        connection.execute_batch(
            "CREATE TABLE meeting_sessions (
               id TEXT PRIMARY KEY,
               status TEXT NOT NULL,
               microphone_enabled INTEGER NOT NULL,
               system_audio_enabled INTEGER NOT NULL,
               stt_provider_id TEXT NOT NULL,
               stt_model_label TEXT NOT NULL,
               translation_provider_id TEXT,
               persistence_mode TEXT NOT NULL,
               started_at TEXT NOT NULL, ended_at TEXT, saved_at TEXT, error_code TEXT
             );
             INSERT INTO meeting_sessions(id,status,microphone_enabled,system_audio_enabled,stt_provider_id,stt_model_label,persistence_mode,started_at)
             VALUES('meeting_recover','active',1,0,'local-whisper','model.bin','discard','1');",
        )
        .expect("legacy meeting fixture");
        connection
            .pragma_update(None, "user_version", 26)
            .expect("v26 fixture");
        initialize_database(&connection).expect("v27 drops meeting tables");
        let exists: bool = connection
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='meeting_sessions')",
                [],
                |row| row.get(0),
            )
            .expect("table check");
        assert!(!exists);
    }
#[test]
    fn version_three_database_migrates_to_four_without_losing_mvp_zero_state() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("v3.sqlite3");
        let connection = Connection::open(&path).expect("database opens");
        initialize_database(&connection).expect("initial schema creates");
        connection
            .execute(
                "INSERT INTO conversations(id, title, task_mode, created_at, updated_at)
             VALUES ('kept-conversation', 'Keep me', 'coding', '1', '1')",
                [],
            )
            .expect("conversation inserts");
        connection
        .execute(
            "INSERT INTO codex_threads(conversation_id, thread_id, model, workspace_path, updated_at)
             VALUES ('kept-conversation', 'kept-thread', 'kept-model', '/tmp/kept', '1')",
            [],
        )
        .expect("thread inserts");
        connection
            .execute(
                "DELETE FROM settings_documents WHERE namespace = 'situation.runtime'",
                [],
            )
            .expect("v4 document removes");
        connection
            .execute("UPDATE settings_documents SET schema_version = 3", [])
            .expect("settings downgrade fixture");
        connection
            .execute(
                "UPDATE settings_documents
                 SET value_json=json_remove(value_json, '$.providers[1]')
                 WHERE namespace='providers.model'",
                [],
            )
            .expect("future provider fixture removes");
        connection
            .pragma_update(None, "user_version", 3)
            .expect("fixture version sets");
        drop(connection);

        let reopened = Connection::open(&path).expect("database reopens");
        initialize_database(&reopened).expect("v4 migration succeeds");
        let version: i64 = reopened
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .expect("version reads");
        assert_eq!(version, crate::persistence::schema::DATABASE_SCHEMA_VERSION);
        let documents = list_settings_documents(&reopened).expect("settings load");
        assert_eq!(documents.len(), 8);
        assert!(documents
            .iter()
            .all(|document| document.namespace == "routing.roles"
                || document.schema_version == SETTINGS_SCHEMA_VERSION));
        let thread: String = reopened
            .query_row(
                "SELECT thread_id FROM codex_threads WHERE conversation_id = 'kept-conversation'",
                [],
                |row| row.get(0),
            )
            .expect("thread remains");
        assert_eq!(thread, "kept-thread");
    }
