#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn audit_triggers_record_relations_without_message_content() {
        let connection = Connection::open_in_memory().expect("database opens");
        crate::initialize_database(&connection).expect("database initializes");
        connection
            .execute(
                "INSERT INTO conversation_messages(id,conversation_id,role,content,created_at)
                 VALUES('message_audit','conversation_primary','user','secret transcript','100')",
                [],
            )
            .expect("message inserts");
        connection
            .execute(
                "INSERT INTO runtime_runs(id,conversation_id,route_kind,status,started_at)
                 VALUES('run_audit','conversation_primary','conversation.respond','running','101')",
                [],
            )
            .expect("run inserts");
        connection
            .execute(
                "INSERT INTO provider_sessions(
                   id,provider_id,runtime_run_id,provider_kind,status,started_at,updated_at
                 ) VALUES(
                   'provider_session_audit','provider_audit','run_audit',
                   'openai-compatible','running','101','101'
                 )",
                [],
            )
            .expect("provider session inserts");
        connection
            .execute(
                "UPDATE provider_sessions SET status='completed',updated_at='102'
                 WHERE id='provider_session_audit'",
                [],
            )
            .expect("provider session finishes");
        connection
            .execute(
                "UPDATE runtime_runs SET status='failed',failure_code='request-timeout',
                 error_message='secret provider detail',completed_at='102' WHERE id='run_audit'",
                [],
            )
            .expect("run finishes");

        let encoded = serde_json::to_string(&recent_events(&connection, 100).expect("events load"))
            .expect("events encode");
        assert!(encoded.contains("message-persisted"));
        assert!(encoded.contains("runtime-run-finished"));
        assert!(encoded.contains("request-timeout"));
        assert!(encoded.contains("run_audit"));
        assert!(!encoded.contains("secret transcript"));
        assert!(!encoded.contains("secret provider detail"));
        let related_provider_events: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM audit_events
                 WHERE component='provider' AND runtime_run_id='run_audit'",
                [],
                |row| row.get(0),
            )
            .expect("provider audit relations load");
        assert_eq!(related_provider_events, 2);
    }

    #[test]
    fn frontend_audit_rejects_freeform_content() {
        let connection = Connection::open_in_memory().expect("database opens");
        crate::initialize_database(&connection).expect("database initializes");
        let input = FrontendAuditEventInput {
            component: "microphone".to_string(),
            event_name: "capture-failed".to_string(),
            phase: "error".to_string(),
            outcome: Some("failure".to_string()),
            correlation_id: Some("voice_session".to_string()),
            causation_id: None,
            conversation_id: None,
            runtime_run_id: None,
            session_id: None,
            subject_id: None,
            failure_code: Some("permission-denied".to_string()),
            attributes: BTreeMap::from([(
                "reasonCode".to_string(),
                AuditAttributeValue::Tag("raw transcript with spaces".to_string()),
            )]),
        };
        assert!(record_event(&connection, &input).is_err());
        let count: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM audit_events WHERE event_name='capture-failed'",
                [],
                |row| row.get(0),
            )
            .expect("audit count loads");
        assert_eq!(count, 0);
    }

    #[test]
    fn voice_asr_channel_audits_metadata_without_transcript_text() {
        let connection = Arc::new(crate::persistence::SqliteWriter::from_connection(
            Connection::open_in_memory().expect("database opens"),
        ));
        crate::initialize_database(&connection.lock().expect("database lock"))
            .expect("database initializes");
        let channel = Channel::new(|_| Ok(()));
        let audited = VoiceAsrAuditChannel::new(
            channel,
            connection.clone(),
            crate::PRIMARY_CONVERSATION_ID.to_string(),
        );
        audited
            .send(VoiceAsrStreamEvent::Partial {
                session_id: "session_audit".to_string(),
                utterance_id: "utterance_audit".to_string(),
                revision: 3,
                start_ms: 100,
                end_ms: 700,
                stable_text: "secret partial".to_string(),
                unstable_text: "secret tail".to_string(),
                language: Some("ja".to_string()),
            })
            .expect("partial sends");
        audited
            .send(VoiceAsrStreamEvent::Final {
                session_id: "session_audit".to_string(),
                utterance_id: "utterance_audit".to_string(),
                revision: 4,
                start_ms: 100,
                end_ms: 900,
                text: "secret recognized speech".to_string(),
                language: Some("ja".to_string()),
            })
            .expect("event sends");

        let connection = connection.lock().expect("database lock");
        let encoded = serde_json::to_string(&recent_events(&connection, 20).expect("events load"))
            .expect("events encode");
        assert!(encoded.contains("asr-final-received"));
        assert!(!encoded.contains("asr-partial-received"));
        assert!(encoded.contains("utterance_audit"));
        assert!(!encoded.contains("durationMs"));
        assert!(!encoded.contains("revision"));
        assert!(!encoded.contains("secret recognized speech"));
        assert!(!encoded.contains("\"ja\""));
    }

    #[test]
    fn turn_request_links_voice_utterance_to_runtime_run() {
        let connection = Connection::open_in_memory().expect("database opens");
        crate::initialize_database(&connection).expect("database initializes");
        let state = crate::test_support::app_state(connection);
        let input = StartTurnInput {
            run_id: "run_voice_audit".to_string(),
            conversation_id: crate::PRIMARY_CONVERSATION_ID.to_string(),
            content: "never persist this prompt in audit".to_string(),
            workspace_path: None,
            retry_input_message_id: None,
            source_id: Some("utterance_voice_audit".to_string()),
            scope_refs: Vec::new(),
            input_origin: "voice".to_string(),
            presentation_mode: "visual".to_string(),
        };
        record_turn_request(&state, &input).expect("turn request audits");

        let connection = state.sqlite_writer.lock().expect("database lock");
        let event = recent_events(&connection, 1)
            .expect("events load")
            .pop()
            .expect("event exists");
        assert_eq!(event["correlationId"], "run_voice_audit");
        assert_eq!(event["causationId"], "utterance_voice_audit");
        assert_eq!(event["runtimeRunId"], "run_voice_audit");
        assert!(!event.to_string().contains("never persist this prompt"));
    }

    #[test]
    fn startup_prunes_only_audit_events_older_than_seven_days() {
        let connection = Connection::open_in_memory().expect("database opens");
        crate::initialize_database(&connection).expect("database initializes");
        let now_ms = now_iso().parse::<i64>().expect("timestamp is milliseconds");
        let eight_days_ago = now_ms - (8 * MILLISECONDS_PER_DAY);
        let six_days_ago = now_ms - (6 * MILLISECONDS_PER_DAY);
        connection
            .execute(
                "INSERT INTO audit_events(id,occurred_at,component,event_name,phase,outcome,attributes_json)
                 VALUES('audit_expired_probe',?1,'app','retention-probe','terminal','success','{}'),
                       ('audit_recent_probe',?2,'app','retention-probe','terminal','success','{}')",
                params![eight_days_ago.to_string(), six_days_ago.to_string()],
            )
            .expect("retention probes insert");

        let before_restart: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM audit_events WHERE id='audit_expired_probe'",
                [],
                |row| row.get(0),
            )
            .expect("pre-startup count loads");
        assert_eq!(before_restart, 1);

        crate::initialize_database(&connection).expect("startup cleanup runs");

        let expired: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM audit_events WHERE id='audit_expired_probe'",
                [],
                |row| row.get(0),
            )
            .expect("expired count loads");
        let recent: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM audit_events WHERE id='audit_recent_probe'",
                [],
                |row| row.get(0),
            )
            .expect("recent count loads");
        assert_eq!(expired, 0);
        assert_eq!(recent, 1);
    }

    #[test]
    fn retention_keeps_events_at_the_seven_day_boundary() {
        let connection = Connection::open_in_memory().expect("database opens");
        crate::initialize_database(&connection).expect("database initializes");
        let now_ms = 10 * MILLISECONDS_PER_DAY;
        let cutoff_ms = now_ms - (AUDIT_RETENTION_DAYS * MILLISECONDS_PER_DAY);
        connection
            .execute(
                "INSERT INTO audit_events(id,occurred_at,component,event_name,phase,outcome,attributes_json)
                 VALUES('audit_before_cutoff',?1,'app','retention-probe','terminal','success','{}'),
                       ('audit_at_cutoff',?2,'app','retention-probe','terminal','success','{}')",
                params![(cutoff_ms - 1).to_string(), cutoff_ms.to_string()],
            )
            .expect("boundary probes insert");

        prune_expired_events(&connection, now_ms).expect("retention cleanup runs");

        let ids = recent_events(&connection, 20)
            .expect("events load")
            .into_iter()
            .filter_map(|event| event["id"].as_str().map(str::to_string))
            .collect::<Vec<_>>();
        assert!(!ids.iter().any(|id| id == "audit_before_cutoff"));
        assert!(ids.iter().any(|id| id == "audit_at_cutoff"));
    }

    #[test]
    fn ui_events_are_bounded_and_newest_first() {
        let connection = Connection::open_in_memory().expect("database opens");
        crate::initialize_database(&connection).expect("database initializes");
        for index in 0..205 {
            connection
                .execute(
                    "INSERT INTO audit_events(id,occurred_at,component,event_name,phase,outcome,attributes_json)
                     VALUES(?1,?2,'app','ui-probe','terminal','success','{}')",
                    params![format!("audit_ui_{index}"), index.to_string()],
                )
                .expect("UI audit fixture inserts");
        }
        let state = crate::test_support::app_state(connection);

        let events = list_ui_events(
            &state,
            AuditEventListInput {
                sort_by: AuditEventSortField::OccurredAt,
                direction: SortDirection::Desc,
            },
        )
        .expect("UI events load");

        assert_eq!(events.len(), AUDIT_UI_EVENT_LIMIT);
        assert_eq!(
            events.first().and_then(|event| event["id"].as_str()),
            Some("audit_ui_204")
        );
        assert_eq!(
            events.last().and_then(|event| event["id"].as_str()),
            Some("audit_ui_5")
        );
    }

    #[test]
    fn ui_events_apply_requested_sort_on_the_bounded_result() {
        let connection = Connection::open_in_memory().expect("database opens");
        crate::initialize_database(&connection).expect("database initializes");
        for (id, component) in [("audit_sort_b", "tts"), ("audit_sort_a", "app")] {
            connection
                .execute(
                    "INSERT INTO audit_events(id,occurred_at,component,event_name,phase,outcome,attributes_json)
                     VALUES(?1,'100',?2,'ui-probe','terminal','success','{}')",
                    params![id, component],
                )
                .expect("UI audit fixture inserts");
        }
        let state = crate::test_support::app_state(connection);

        let events = list_ui_events(
            &state,
            AuditEventListInput {
                sort_by: AuditEventSortField::Component,
                direction: SortDirection::Asc,
            },
        )
        .expect("sorted UI events load");

        let positions = events
            .iter()
            .filter_map(|event| event["id"].as_str())
            .filter(|id| id.starts_with("audit_sort_"))
            .collect::<Vec<_>>();
        assert_eq!(positions, vec!["audit_sort_a", "audit_sort_b"]);
    }
}
