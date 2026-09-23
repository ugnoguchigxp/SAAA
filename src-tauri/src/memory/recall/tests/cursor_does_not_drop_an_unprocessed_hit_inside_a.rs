use super::*;
use crate::memory::contracts::{
    RecallConversationInput, RecallConversationOutput, RecallError, RecallErrorCode,
    RecallTimeFilter, RecallTimePreset, MAX_RECALL_CALLS_PER_TURN, RECALL_NOTICE,
    RECALL_RETRIEVAL_MODE,
};
use chrono::{
    DateTime, Datelike, Duration, LocalResult, Months, NaiveDate, TimeZone, Utc, Weekday,
};
use chrono_tz::Tz;
use rusqlite::{params, Connection, OptionalExtension};
use super::super::search::*;
#[test]
    pub(super) fn cursor_does_not_drop_an_unprocessed_hit_inside_a_previous_window() {
        let mut connection = database();
        insert_turn(&connection, "history", 1, 1_000, "opening", "answer");
        for index in 0..6 {
            connection
                .execute(
                    "INSERT INTO conversation_messages(id,conversation_id,role,content,created_at)
                     VALUES(?1,'history','transcript',?2,?3)",
                    params![
                        format!("overlap_{index}"),
                        format!("overlap-key {index}"),
                        (1_002 + index).to_string()
                    ],
                )
                .expect("overlapping hit inserts");
        }
        insert_turn(&connection, "current", 99, 2_000, "current", "answer");
        start_run(&connection, "message_99_user");
        let now = Utc
            .timestamp_millis_opt(3_000)
            .single()
            .expect("fixture time");

        let first = execute(
            &mut connection,
            RecallExecutionContext {
                runtime_run_id: "run_recall",
                tool_call_id: "call_overlap_1",
                now,
                timezone: chrono_tz::UTC,
            },
            RecallConversationInput {
                query: Some("overlap-key".to_string()),
                time: None,
                cursor: None,
            },
        )
        .expect("first page succeeds");
        assert_eq!(first.windows[0].matched_event_refs.len(), 5);
        let second = execute(
            &mut connection,
            RecallExecutionContext {
                runtime_run_id: "run_recall",
                tool_call_id: "call_overlap_2",
                now,
                timezone: chrono_tz::UTC,
            },
            RecallConversationInput {
                query: Some("overlap-key".to_string()),
                time: None,
                cursor: first.next_cursor,
            },
        )
        .expect("second page succeeds");

        assert_eq!(second.windows.len(), 1);
        assert_eq!(
            second.windows[0].matched_event_refs,
            vec![opaque_ref("event", "overlap_0")]
        );
        assert!(second.next_cursor.is_none());
    }
#[test]
    pub(super) fn query_metacharacters_remain_plain_text_and_never_become_sql_or_fts_syntax() {
        let mut connection = database();
        insert_turn(
            &connection,
            "history",
            1,
            1_000,
            "SQLite ordinary discussion",
            "ordinary answer",
        );
        insert_turn(&connection, "current", 2, 2_000, "current", "answer");
        start_run(&connection, "message_2_user");
        let output = execute(
            &mut connection,
            RecallExecutionContext {
                runtime_run_id: "run_recall",
                tool_call_id: "call_metacharacters",
                now: Utc
                    .timestamp_millis_opt(3_000)
                    .single()
                    .expect("fixture time"),
                timezone: chrono_tz::UTC,
            },
            RecallConversationInput {
                query: Some("SQLite\" OR *; DROP TABLE conversation_messages --".to_string()),
                time: None,
                cursor: None,
            },
        )
        .expect("metacharacters are handled as bounded plain text");
        assert_eq!(output.reason_code, "continuity-no-hit");
        let messages_table_exists: bool = connection
            .query_row(
                "SELECT EXISTS(
                   SELECT 1 FROM sqlite_master
                   WHERE type='table' AND name='conversation_messages'
                 )",
                [],
                |row| row.get(0),
            )
            .expect("schema reads");
        assert!(messages_table_exists);
    }
#[test]
    pub(super) fn absolute_ranges_and_call_limits_are_enforced() {
        let mut connection = database();
        insert_turn(&connection, "current", 1, 1_000, "current", "answer");
        start_run(&connection, "message_1_user");
        let now = Utc
            .with_ymd_and_hms(2026, 8, 29, 0, 0, 0)
            .single()
            .expect("fixture time");
        let invalid = execute(
            &mut connection,
            RecallExecutionContext {
                runtime_run_id: "run_recall",
                tool_call_id: "call_invalid",
                now,
                timezone: chrono_tz::UTC,
            },
            RecallConversationInput {
                query: None,
                time: Some(RecallTimeFilter::Absolute {
                    from: "2025-01-01T00:00:00Z".to_string(),
                    to_exclusive: "2026-08-29T00:00:00Z".to_string(),
                }),
                cursor: None,
            },
        )
        .expect_err("oversized range is rejected");
        assert_eq!(invalid.code, RecallErrorCode::InvalidTimeRange);

        for index in 0..2 {
            execute(
                &mut connection,
                RecallExecutionContext {
                    runtime_run_id: "run_recall",
                    tool_call_id: &format!("call_{index}"),
                    now,
                    timezone: chrono_tz::UTC,
                },
                RecallConversationInput {
                    query: Some("missing".to_string()),
                    time: None,
                    cursor: None,
                },
            )
            .expect("bounded no-hit call succeeds");
        }
        let limit = execute(
            &mut connection,
            RecallExecutionContext {
                runtime_run_id: "run_recall",
                tool_call_id: "call_3",
                now,
                timezone: chrono_tz::UTC,
            },
            RecallConversationInput {
                query: Some("missing".to_string()),
                time: None,
                cursor: None,
            },
        )
        .expect_err("fourth call is rejected");
        assert_eq!(limit.code, RecallErrorCode::CallLimitExceeded);
        let (attempts, receipts): (i64, i64) = (
            connection
                .query_row(
                    "SELECT COUNT(*) FROM conversation_recall_attempts",
                    [],
                    |row| row.get(0),
                )
                .expect("attempt count reads"),
            connection
                .query_row(
                    "SELECT COUNT(*) FROM conversation_recall_receipts",
                    [],
                    |row| row.get(0),
                )
                .expect("receipt count reads"),
        );
        assert_eq!(attempts, 3, "invalid attempts count toward the hard cap");
        assert_eq!(receipts, 2, "only completed searches create receipts");

        let sub_millisecond = resolve_time(
            &RecallTimeFilter::Absolute {
                from: "2026-08-29T00:00:00.0001Z".to_string(),
                to_exclusive: "2026-08-29T00:00:00.0002Z".to_string(),
            },
            now,
            chrono_tz::UTC,
        )
        .expect_err("a range empty at storage precision is rejected");
        assert_eq!(sub_millisecond.code, RecallErrorCode::InvalidTimeRange);
    }
#[test]
    pub(super) fn duplicate_tool_call_ids_still_consume_the_turn_call_limit() {
        let mut connection = database();
        insert_turn(&connection, "current", 1, 1_000, "current", "answer");
        start_run(&connection, "message_1_user");
        let now = Utc
            .timestamp_millis_opt(2_000)
            .single()
            .expect("fixture time");
        let input = RecallConversationInput {
            query: Some("missing".to_string()),
            time: None,
            cursor: None,
        };
        execute(
            &mut connection,
            RecallExecutionContext {
                runtime_run_id: "run_recall",
                tool_call_id: "call_duplicate",
                now,
                timezone: chrono_tz::UTC,
            },
            input.clone(),
        )
        .expect("first call succeeds");
        for _ in 0..2 {
            let duplicate = execute(
                &mut connection,
                RecallExecutionContext {
                    runtime_run_id: "run_recall",
                    tool_call_id: "call_duplicate",
                    now,
                    timezone: chrono_tz::UTC,
                },
                input.clone(),
            )
            .expect_err("duplicate call is rejected");
            assert_eq!(duplicate.code, RecallErrorCode::InvalidInput);
        }
        assert_eq!(
            remaining_calls(&connection, "run_recall").expect("remaining calls read"),
            0
        );
        let limit = execute(
            &mut connection,
            RecallExecutionContext {
                runtime_run_id: "run_recall",
                tool_call_id: "call_after_duplicates",
                now,
                timezone: chrono_tz::UTC,
            },
            input,
        )
        .expect_err("fourth invocation is rejected");
        assert_eq!(limit.code, RecallErrorCode::CallLimitExceeded);
    }
#[test]
    pub(super) fn time_only_search_resolves_yesterday_in_the_supplied_timezone() {
        let mut connection = database();
        let timezone = chrono_tz::Asia::Tokyo;
        let now = Utc
            .with_ymd_and_hms(2026, 8, 29, 3, 30, 0)
            .single()
            .expect("fixture time");
        let yesterday = Utc
            .with_ymd_and_hms(2026, 8, 28, 4, 0, 0)
            .single()
            .expect("yesterday time")
            .timestamp_millis();
        insert_turn(
            &connection,
            "history",
            1,
            yesterday,
            "時間だけで探せる会話",
            "探せます",
        );
        insert_turn(
            &connection,
            "current",
            2,
            now.timestamp_millis(),
            "昨日の会話を見せて",
            "未回答",
        );
        start_run(&connection, "message_2_user");

        let output = execute(
            &mut connection,
            RecallExecutionContext {
                runtime_run_id: "run_recall",
                tool_call_id: "call_time_only",
                now,
                timezone,
            },
            RecallConversationInput {
                query: None,
                time: Some(RecallTimeFilter::Preset {
                    preset: RecallTimePreset::Yesterday,
                }),
                cursor: None,
            },
        )
        .expect("time-only recall succeeds");

        assert_eq!(output.reason_code, "ok");
        assert_eq!(
            output
                .resolved_time_range
                .as_ref()
                .expect("range resolves")
                .from,
            "2026-08-28T00:00:00+09:00"
        );
        assert!(output
            .windows
            .iter()
            .flat_map(|window| &window.events)
            .any(|event| event.content == "時間だけで探せる会話"));
        assert!(!output
            .windows
            .iter()
            .flat_map(|window| &window.events)
            .any(|event| event.content == "昨日の会話を見せて"));
    }
#[test]
    pub(super) fn cursor_keeps_the_original_time_range_across_local_midnight() {
        let mut connection = database();
        let timezone = chrono_tz::Asia::Tokyo;
        let before_midnight = Utc
            .with_ymd_and_hms(2026, 8, 29, 14, 30, 0)
            .single()
            .expect("fixture time");
        let historical = Utc
            .with_ymd_and_hms(2026, 8, 28, 3, 0, 0)
            .single()
            .expect("historical time")
            .timestamp_millis();
        for sequence in 0..5 {
            insert_turn(
                &connection,
                &format!("history_{sequence}"),
                sequence,
                historical + i64::try_from(sequence).expect("sequence"),
                &format!("pagination-key {sequence}"),
                "neighbor",
            );
        }
        insert_turn(
            &connection,
            "current",
            99,
            before_midnight.timestamp_millis(),
            "過去の続き",
            "未回答",
        );
        start_run(&connection, "message_99_user");
        let filter = RecallTimeFilter::Preset {
            preset: RecallTimePreset::Yesterday,
        };
        let first = execute(
            &mut connection,
            RecallExecutionContext {
                runtime_run_id: "run_recall",
                tool_call_id: "call_page_1",
                now: before_midnight,
                timezone,
            },
            RecallConversationInput {
                query: Some("pagination-key".to_string()),
                time: Some(filter.clone()),
                cursor: None,
            },
        )
        .expect("first page succeeds");
        assert_eq!(first.windows.len(), 3);
        let cursor = first.next_cursor.clone().expect("next cursor exists");

        insert_turn(
            &connection,
            "late_history",
            77,
            historical + 100,
            "pagination-key inserted after page one",
            "must not enter this cursor snapshot",
        );

        let after_midnight = before_midnight + Duration::hours(2);
        let second = execute(
            &mut connection,
            RecallExecutionContext {
                runtime_run_id: "run_recall",
                tool_call_id: "call_page_2",
                now: after_midnight,
                timezone,
            },
            RecallConversationInput {
                query: Some("pagination-key".to_string()),
                time: Some(filter),
                cursor: Some(cursor.clone()),
            },
        )
        .expect("second page succeeds");
        assert_eq!(second.windows.len(), 2);
        assert_eq!(second.resolved_time_range, first.resolved_time_range);
        assert!(second.next_cursor.is_none());
        let first_refs = first
            .windows
            .iter()
            .flat_map(|window| window.events.iter().map(|event| event.event_ref.clone()))
            .collect::<HashSet<_>>();
        assert!(second
            .windows
            .iter()
            .flat_map(|window| &window.events)
            .all(|event| !first_refs.contains(&event.event_ref)));
        assert!(!second
            .windows
            .iter()
            .flat_map(|window| &window.events)
            .any(|event| event.content.contains("inserted after page one")));

        let mismatch = execute(
            &mut connection,
            RecallExecutionContext {
                runtime_run_id: "run_recall",
                tool_call_id: "call_page_mismatch",
                now: after_midnight,
                timezone,
            },
            RecallConversationInput {
                query: Some("different-key".to_string()),
                time: Some(RecallTimeFilter::Preset {
                    preset: RecallTimePreset::Yesterday,
                }),
                cursor: Some(cursor),
            },
        )
        .expect_err("cursor cannot be reused with different filters");
        assert_eq!(mismatch.code, RecallErrorCode::CursorFilterMismatch);
    }
