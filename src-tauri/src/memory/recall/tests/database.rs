use rusqlite::params;
use std::collections::HashSet;
use chrono::TimeZone as _;
use super::*;
pub(crate) fn database() -> Connection {
        let connection = Connection::open_in_memory().expect("database opens");
        connection
            .execute_batch(
                "PRAGMA foreign_keys=ON;
                 CREATE TABLE conversations(
                   id TEXT PRIMARY KEY,title TEXT,task_mode TEXT,created_at TEXT,updated_at TEXT
                 );
                 CREATE TABLE conversation_messages(
                   id TEXT PRIMARY KEY,conversation_id TEXT NOT NULL,role TEXT NOT NULL,
                   content TEXT NOT NULL,created_at TEXT NOT NULL
                 );
                 CREATE TABLE runtime_runs(
                   id TEXT PRIMARY KEY,conversation_id TEXT NOT NULL,status TEXT NOT NULL,
                   input_message_id TEXT
                 );",
            )
            .expect("base schema creates");
        migrate_v9_to_v10(&connection).expect("recall schema migrates");
        connection
    }
pub(crate) fn insert_turn(
        connection: &Connection,
        conversation_id: &str,
        sequence: usize,
        created_at_ms: i64,
        user: &str,
        assistant: &str,
    ) {
        connection
            .execute(
                "INSERT OR IGNORE INTO conversations(id,task_mode,created_at,updated_at)
                 VALUES(?1,'conversation','0','0')",
                [conversation_id],
            )
            .expect("conversation inserts");
        connection
            .execute(
                "INSERT INTO conversation_messages(id,conversation_id,role,content,created_at)
                 VALUES(?1,?2,'user',?3,?4)",
                params![
                    format!("message_{sequence}_user"),
                    conversation_id,
                    user,
                    created_at_ms.to_string()
                ],
            )
            .expect("user inserts");
        connection
            .execute(
                "INSERT INTO conversation_messages(id,conversation_id,role,content,created_at)
                 VALUES(?1,?2,'assistant',?3,?4)",
                params![
                    format!("message_{sequence}_assistant"),
                    conversation_id,
                    assistant,
                    created_at_ms.saturating_add(1).to_string()
                ],
            )
            .expect("assistant inserts");
    }
pub(crate) fn start_run(connection: &Connection, current_message_id: &str) {
        connection
            .execute(
                "INSERT INTO runtime_runs(id,conversation_id,status,input_message_id)
                 VALUES('run_recall','current','running',?1)",
                [current_message_id],
            )
            .expect("run inserts");
    }
#[test]
    pub(crate) fn relative_time_presets_use_calendar_boundaries_and_rolling_days() {
        let now = Utc
            .with_ymd_and_hms(2026, 8, 29, 3, 30, 0)
            .single()
            .expect("fixture time");
        let timezone = chrono_tz::Asia::Tokyo;
        let yesterday = resolve_time(
            &RecallTimeFilter::Preset {
                preset: RecallTimePreset::Yesterday,
            },
            now,
            timezone,
        )
        .expect("yesterday resolves");
        assert_eq!(public_range(&yesterday).from, "2026-08-28T00:00:00+09:00");
        assert_eq!(
            public_range(&yesterday).to_exclusive,
            "2026-08-29T00:00:00+09:00"
        );
        let previous_week = resolve_time(
            &RecallTimeFilter::Preset {
                preset: RecallTimePreset::PreviousCalendarWeek,
            },
            now,
            timezone,
        )
        .expect("previous week resolves");
        assert_eq!(
            public_range(&previous_week).from,
            "2026-08-17T00:00:00+09:00"
        );
        assert_eq!(
            public_range(&previous_week).to_exclusive,
            "2026-08-24T00:00:00+09:00"
        );
        let rolling = resolve_time(
            &RecallTimeFilter::Preset {
                preset: RecallTimePreset::Past7Days,
            },
            now,
            timezone,
        )
        .expect("rolling range resolves");
        assert_ne!(rolling.from_ms, previous_week.from_ms);

        let leap_month = resolve_time(
            &RecallTimeFilter::Preset {
                preset: RecallTimePreset::PreviousCalendarMonth,
            },
            Utc.with_ymd_and_hms(2024, 3, 1, 3, 0, 0)
                .single()
                .expect("leap fixture time"),
            timezone,
        )
        .expect("leap month resolves");
        assert_eq!(public_range(&leap_month).from, "2024-02-01T00:00:00+09:00");
        assert_eq!(
            public_range(&leap_month).to_exclusive,
            "2024-03-01T00:00:00+09:00"
        );

        let dst_timezone = chrono_tz::America::New_York;
        let dst_yesterday = resolve_time(
            &RecallTimeFilter::Preset {
                preset: RecallTimePreset::Yesterday,
            },
            Utc.with_ymd_and_hms(2026, 3, 9, 12, 0, 0)
                .single()
                .expect("DST fixture time"),
            dst_timezone,
        )
        .expect("DST day resolves");
        assert_eq!(
            dst_yesterday.to_exclusive_ms - dst_yesterday.from_ms,
            Duration::hours(23).num_milliseconds()
        );

        assert!(start_of_day(
            chrono_tz::Pacific::Apia,
            NaiveDate::from_ymd_opt(2011, 12, 30).expect("skipped date fixture")
        )
        .is_err());
    }
#[test]
    pub(crate) fn projected_relevance_increases_with_stronger_sqlite_bm25_matches() {
        assert!(score(-2.0) > score(-1.0));
        assert!(score(-1.0) > score(0.0));
        assert_eq!(score(f64::NAN), 0.0);
    }
#[test]
    pub(crate) fn version_ten_schema_is_repaired_without_rebuilding_the_database() {
        let connection = database();
        insert_turn(
            &connection,
            "repair_history",
            1,
            1_000,
            "repair keyword",
            "repair answer",
        );
        connection
            .execute_batch(
                "PRAGMA user_version=10;
                 DELETE FROM conversation_messages_fts;
                 DROP TABLE conversation_recall_attempts;
                 CREATE TABLE conversation_recall_attempts (
                   id TEXT PRIMARY KEY,
                   runtime_run_id TEXT NOT NULL,
                   tool_call_id TEXT NOT NULL,
                   call_index INTEGER NOT NULL,
                   created_at_ms INTEGER NOT NULL,
                   UNIQUE(runtime_run_id,tool_call_id),
                   UNIQUE(runtime_run_id,call_index)
                 );
                 DROP TRIGGER conversation_messages_recall_insert;
                 DROP TABLE conversation_recall_cursors;
                 CREATE TABLE conversation_recall_cursors (
                   id TEXT PRIMARY KEY,
                   runtime_run_id TEXT NOT NULL,
                   filter_digest TEXT NOT NULL,
                   candidate_offset INTEGER NOT NULL,
                   range_from_ms INTEGER,
                   range_to_exclusive_ms INTEGER,
                   timezone TEXT,
                   range_label TEXT,
                   expires_at_ms INTEGER NOT NULL,
                   created_at_ms INTEGER NOT NULL
                 );",
            )
            .expect("old v10 fixture creates");

        migrate_v9_to_v10(&connection).expect("v10 schema repairs");
        let attempts: bool = connection
            .query_row(
                "SELECT EXISTS(
                   SELECT 1 FROM sqlite_master
                   WHERE type='table' AND name='conversation_recall_attempts'
                 )",
                [],
                |row| row.get(0),
            )
            .expect("attempt table reads");
        let insert_trigger: bool = connection
            .query_row(
                "SELECT EXISTS(
                   SELECT 1 FROM sqlite_master
                   WHERE type='trigger' AND name='conversation_messages_recall_insert'
                 )",
                [],
                |row| row.get(0),
            )
            .expect("trigger reads");
        let snapshot_column: bool = connection
            .query_row(
                "SELECT EXISTS(
                   SELECT 1 FROM pragma_table_info('conversation_recall_cursors')
                   WHERE name='snapshot_max_rowid'
                 )",
                [],
                |row| row.get(0),
            )
            .expect("cursor columns read");
        let attempts_schema: String = connection
            .query_row(
                "SELECT sql FROM sqlite_master
                 WHERE type='table' AND name='conversation_recall_attempts'",
                [],
                |row| row.get(0),
            )
            .expect("attempt schema reads");
        let fts_rows: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM conversation_messages_fts",
                [],
                |row| row.get(0),
            )
            .expect("fts rows read");
        assert!(attempts);
        assert!(insert_trigger);
        assert!(snapshot_column);
        assert_eq!(fts_rows, 2);
        assert!(!attempts_schema
            .split_whitespace()
            .collect::<String>()
            .contains("UNIQUE(runtime_run_id,tool_call_id)"));
    }
#[test]
    pub(crate) fn keyword_and_time_search_returns_bounded_neighbor_turns_without_current_message() {
        let mut connection = database();
        let timezone = chrono_tz::Asia::Tokyo;
        let now = Utc
            .with_ymd_and_hms(2026, 8, 29, 3, 30, 0)
            .single()
            .expect("fixture time");
        let yesterday_ms = Utc
            .with_ymd_and_hms(2026, 8, 28, 1, 0, 0)
            .single()
            .expect("yesterday time")
            .timestamp_millis();
        for sequence in 0..6 {
            insert_turn(
                &connection,
                "history",
                sequence,
                yesterday_ms + i64::try_from(sequence).expect("sequence") * 10,
                if sequence == 3 {
                    "SQLiteの保存方針"
                } else {
                    "別の話題"
                },
                &format!("回答{sequence}"),
            );
        }
        insert_turn(
            &connection,
            "current",
            99,
            now.timestamp_millis(),
            "昨日SQLiteの保存方針について何を話した？",
            "未回答",
        );
        start_run(&connection, "message_99_user");
        let output = execute(
            &mut connection,
            RecallExecutionContext {
                runtime_run_id: "run_recall",
                tool_call_id: "call_1",
                now,
                timezone,
            },
            RecallConversationInput {
                query: Some("SQLite 保存方針".to_string()),
                time: Some(RecallTimeFilter::Preset {
                    preset: RecallTimePreset::Yesterday,
                }),
                cursor: None,
            },
        )
        .expect("recall succeeds");
        assert_eq!(output.reason_code, "ok");
        assert_eq!(output.windows.len(), 1);
        let events = &output.windows[0].events;
        assert!(events.iter().any(|event| event.content.contains("SQLite")));
        assert!(!events
            .iter()
            .any(|event| event.event_ref == "message_99_user"));
        assert!(events.len() <= 10);
        let raw_result_count: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM conversation_messages WHERE content LIKE '%retrievalMode%'",
                [],
                |row| row.get(0),
            )
            .expect("count reads");
        assert_eq!(raw_result_count, 0);
        let receipt_count: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM conversation_recall_receipts",
                [],
                |row| row.get(0),
            )
            .expect("receipt count reads");
        assert_eq!(receipt_count, 1);
    }
#[test]
    pub(crate) fn raw_windows_exclude_system_messages_and_bound_dense_turns() {
        let mut connection = database();
        insert_turn(
            &connection,
            "history",
            1,
            1_000,
            "dense-anchor-key",
            "first answer",
        );
        connection
            .execute(
                "INSERT INTO conversation_messages(id,conversation_id,role,content,created_at)
                 VALUES('old-system','history','system','obsolete hidden instruction','1002')",
                [],
            )
            .expect("system message inserts");
        for index in 0..100 {
            connection
                .execute(
                    "INSERT INTO conversation_messages(id,conversation_id,role,content,created_at)
                     VALUES(?1,'history','transcript',?2,?3)",
                    params![
                        format!("dense_transcript_{index}"),
                        format!("transcript {index}"),
                        (1_003 + index).to_string()
                    ],
                )
                .expect("dense transcript inserts");
        }
        insert_turn(&connection, "current", 2, 2_000, "current", "answer");
        start_run(&connection, "message_2_user");
        let snapshot_max_rowid: i64 = connection
            .query_row("SELECT MAX(rowid) FROM conversation_messages", [], |row| {
                row.get(0)
            })
            .expect("snapshot reads");
        let candidate = search_candidates(
            &connection,
            Some("dense-anchor-key"),
            None,
            "message_2_user",
            snapshot_max_rowid,
            0,
            1,
        )
        .expect("candidate search succeeds")
        .pop()
        .expect("anchor exists");
        let window = load_window(
            &connection,
            &candidate,
            None,
            "message_2_user",
            snapshot_max_rowid,
        )
        .expect("window loads");
        assert!(window.events.len() <= MAX_EVENTS_PER_WINDOW);
        assert!(window.events.iter().any(|event| event.id == candidate.id));
        assert!(window.events.iter().all(|event| event.role != "system"));

        let output = execute(
            &mut connection,
            RecallExecutionContext {
                runtime_run_id: "run_recall",
                tool_call_id: "call_dense",
                now: Utc
                    .timestamp_millis_opt(3_000)
                    .single()
                    .expect("fixture time"),
                timezone: chrono_tz::UTC,
            },
            RecallConversationInput {
                query: Some("dense-anchor-key".to_string()),
                time: None,
                cursor: None,
            },
        )
        .expect("bounded recall succeeds");
        assert!(!output
            .windows
            .iter()
            .flat_map(|window| &window.events)
            .any(|event| event.content.contains("obsolete hidden instruction")));
    }
#[test]
    pub(crate) fn projection_preserves_every_anchor_within_the_shared_token_budget() {
        let events = (0..5)
            .map(|index| InternalEvent {
                id: format!("anchor_{index}"),
                conversation_id: "history".to_string(),
                role: "user".to_string(),
                content: "長".repeat(2_000),
                created_at_ms: i64::from(index),
                rowid: i64::from(index),
                turn_sequence: i64::from(index),
            })
            .collect::<Vec<_>>();
        let (windows, truncated) = project_windows(
            vec![InternalWindow {
                conversation_id: "history".to_string(),
                first_turn: 0,
                last_turn: 4,
                score: 1.0,
                matched_event_refs: events.iter().map(|event| event.id.clone()).collect(),
                events,
            }],
            chrono_tz::UTC,
        );

        assert!(truncated);
        assert_eq!(windows.len(), 1);
        assert_eq!(windows[0].matched_event_refs.len(), 5);
        assert_eq!(windows[0].events.len(), 5);
        assert!(windows[0]
            .events
            .iter()
            .all(|event| event.truncated && !event.content.is_empty()));
        assert!(
            windows[0]
                .events
                .iter()
                .map(|event| event.content.len())
                .sum::<usize>()
                <= MAX_OUTPUT_TOKEN_BUDGET
        );
    }
