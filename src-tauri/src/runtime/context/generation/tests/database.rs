use rusqlite::params;
use super::*;
pub(super) fn database() -> Connection {
        let connection = Connection::open_in_memory().expect("database opens");
        crate::persistence::schema::initialize_database(&connection).expect("schema initializes");
        connection
            .execute(
                "INSERT INTO conversation_messages(id,conversation_id,role,content,created_at)
                 VALUES('input',?1,'user','secret current request','1')",
                [crate::PRIMARY_CONVERSATION_ID],
            )
            .expect("input inserts");
        connection
            .execute(
                "INSERT INTO runtime_runs(id,conversation_id,route_kind,status,input_message_id,started_at)
                 VALUES('run',?1,'conversation.respond','running','input','1')",
                [crate::PRIMARY_CONVERSATION_ID],
            )
            .expect("run inserts");
        connection
    }
#[test]
    pub(super) fn final_wire_budget_never_demotes_required_context_to_a_normal_size_failure() {
        assert_eq!(
            final_wire_size(MAX_PROVIDER_CONTEXT_WIRE_BYTES + 1, true),
            FinalWireSize::RequiredContextOverflow
        );
        assert_eq!(
            final_wire_size(MAX_PROVIDER_CONTEXT_WIRE_BYTES + 1, false),
            FinalWireSize::Fits
        );
        assert_eq!(
            final_wire_size(MAX_PROVIDER_REQUEST_BYTES + 1, false),
            FinalWireSize::RequestTooLarge
        );
    }
pub(super) fn bind_scope(connection: &Connection) {
        connection
            .execute(
                "INSERT INTO context_scopes(scope_key,kind,opaque_id,state,created_at)
                 VALUES('scope','project','opaque','active','1')",
                [],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO context_scope_epochs(scope_key,epoch) VALUES('scope',0)",
                [],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO runtime_scope_resolutions(
                   run_id,status,focus_scope_key,scope_digest,resolved_at
                 ) VALUES('run','resolved','scope',?1,'1')",
                [digest(b"scope")],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO runtime_run_scopes(run_id,scope_key,relation,source,epoch)
                 VALUES('run','scope','current','runtime',0)",
                [],
            )
            .unwrap();
    }
pub(super) fn bind_personal_assertion(connection: &Connection) {
        connection
            .execute(
                "INSERT INTO personal_payloads(id,value_json,bytes)
                 VALUES('payload','{}',2)",
                [],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO personal_assertions(id,metadata,payload_id,erased)
                 VALUES('assertion','{}','payload',0)",
                [],
            )
            .unwrap();
    }
pub(super) fn bind_selected_assertion(generation: &GenerationHandle) {
        generation
            .add_input(
                "personal-state",
                "assertion",
                1,
                &digest(b"assertion"),
                "must",
                "base",
                true,
                None,
            )
            .unwrap();
    }
pub(super) fn insert_planned_generation(
        connection: &Connection,
        source_kind: &str,
        source_id: &str,
        version: u64,
    ) {
        connection.execute(
            "INSERT INTO context_generations(
               id,run_id,provider_id,ordinal,purpose,envelope_digest,request_digest,
               projected_bytes,current_instruction_count,health_status,status,started_at
             ) VALUES('continuation-generation','run','provider',1,'reasoning',?1,?1,42,1,'green','planned','1')",
            [digest(b"request")],
        ).unwrap();
        connection.execute(
            "INSERT INTO context_generation_inputs(
               generation_id,source_kind,source_id,source_version,source_digest,
               requirement,placement,selected,metadata_json
             ) VALUES('continuation-generation','current-instruction','input',1,?1,'must','base',1,'{}')",
            [digest(b"secret current request")],
        ).unwrap();
        connection
            .execute(
                "INSERT INTO context_generation_inputs(
               generation_id,source_kind,source_id,source_version,source_digest,
               requirement,placement,selected,metadata_json
             ) VALUES('continuation-generation',?1,?2,?3,?4,'must','base',1,'{}')",
                params![
                    source_kind,
                    source_id,
                    version,
                    digest(source_id.as_bytes())
                ],
            )
            .unwrap();
    }
#[test]
    pub(super) fn delegation_withdrawal_before_dispatch_rejects_the_generation_cas() {
        let connection = database();
        connection.execute_batch(
            "INSERT INTO steward_goals(id,conversation_id,origin,success_condition,status,created_at,summary,revision,verifier)
               VALUES('goal', 'conversation-primary','user_explicit','verify','active','1','read docs',1,'user_confirmation_required');
             INSERT INTO steward_delegations(id,goal_id,conversation_id,workspace_id,ops,budget_runs,budget_ms,notify,status,created_at,revision)
               VALUES('delegation','goal','conversation-primary','workspace','read',1,1,'silent','active','1',1);
             INSERT INTO steward_tasks(id,delegation_id,conversation_id,trigger_kind,source_id,dedupe_key,loop_state,created_at,updated_at,revision)
               VALUES('task','delegation','conversation-primary','start','input','key','running','1','1',3);",
        ).unwrap();
        insert_planned_generation(&connection, "delegation-continuation", "task", 3);
        assert!(validate_dependencies(&connection, "continuation-generation").is_ok());
        connection
            .execute(
                "UPDATE steward_delegations SET status='withdrawn' WHERE id='delegation'",
                [],
            )
            .unwrap();
        assert!(
            validate_dependencies(&connection, "continuation-generation")
                .unwrap_err()
                .contains("delegation continuation changed")
        );
    }
#[test]
    pub(super) fn completed_task_before_dispatch_rejects_the_generation_cas() {
        let connection = database();
        connection.execute_batch(
            "INSERT INTO coding_jobs(id,conversation_id,source_id,workspace_id,workspace_path,settings_json,revision,session_path,state,current_run_id)
               VALUES('coding-job','conversation-primary','input','workspace','/tmp','{}',4,'/tmp/session','running','coding-run');
             INSERT INTO coding_runs(id,job_id,source_id,host_run_id,payload,digest,delivery,state,started_at)
               VALUES('coding-run','coding-job','input','host','{}','digest','accepted','running','1');",
        ).unwrap();
        insert_planned_generation(&connection, "task-continuation", "coding-job", 4);
        assert!(validate_dependencies(&connection, "continuation-generation").is_ok());
        connection
            .execute(
                "UPDATE coding_runs SET state='settled' WHERE id='coding-run'",
                [],
            )
            .unwrap();
        assert!(
            validate_dependencies(&connection, "continuation-generation")
                .unwrap_err()
                .contains("task continuation changed")
        );
    }
#[test]
    pub(super) fn schema_records_only_digests_and_one_current_source() {
        let mut connection = database();
        let transaction = connection.transaction().unwrap();
        transaction
            .execute(
                "INSERT INTO context_generations(
                   id,run_id,provider_id,ordinal,purpose,envelope_digest,request_digest,
                   projected_bytes,current_instruction_count,health_status,status,started_at
                 ) VALUES('generation','run','provider',1,'reasoning',?1,?1,42,1,'green','planned','1')",
                [digest(b"request")],
            )
            .unwrap();
        transaction
            .execute(
                "INSERT INTO context_generation_inputs(
                   generation_id,source_kind,source_id,source_version,source_digest,
                   requirement,placement,selected,metadata_json
                 ) VALUES('generation','current-instruction','input',1,?1,'must','base',1,'{}')",
                [digest(b"secret current request")],
            )
            .unwrap();
        transaction.commit().unwrap();

        let stored: String = connection
            .query_row(
                "SELECT envelope_digest || request_digest FROM context_generations WHERE id='generation'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(!stored.contains("secret current request"));
        assert_eq!(stored.len(), 128);
    }
#[test]
    pub(super) fn required_receipt_is_written_before_dispatch_without_context_text() {
        let state = crate::test_support::app_state(database());
        let generation = begin(
            &state,
            BeginGeneration {
                run_id: "run",
                provider_session_id: None,
                provider_id: Some("provider"),
                purpose: "reasoning",
                request_payload: b"serialized final provider body",
                envelope_payload: b"envelope",
                current_instruction_count: 1,
            },
        )
        .unwrap();
        let required = digest(b"ordered required candidate identities");
        generation.set_required_receipt(&required).unwrap();
        generation.dispatch().unwrap();
        state
            .sqlite_readers
            .read(|connection| {
                let receipt: (String, String) = connection
                    .query_row(
                        "SELECT required_set_digest,request_digest
                         FROM context_generations WHERE run_id='run'",
                        [],
                        |row| Ok((row.get(0)?, row.get(1)?)),
                    )
                    .map_err(database_error)?;
                assert_eq!(
                    receipt,
                    (required, digest(b"serialized final provider body"))
                );
                Ok(())
            })
            .unwrap();
    }
#[test]
    pub(super) fn dropped_generation_handles_are_terminalized() {
        let state = crate::test_support::app_state(database());
        let generation = begin(
            &state,
            BeginGeneration {
                run_id: "run",
                provider_session_id: None,
                provider_id: Some("provider"),
                purpose: "reasoning",
                request_payload: b"request",
                envelope_payload: b"request",
                current_instruction_count: 1,
            },
        )
        .unwrap();
        generation.dispatch().unwrap();
        drop(generation);
        state
            .sqlite_readers
            .read(|connection| {
                let status: String = connection
                    .query_row(
                        "SELECT status FROM context_generations WHERE run_id='run'",
                        [],
                        |row| row.get(0),
                    )
                    .map_err(database_error)?;
                assert_eq!(status, "interrupted");
                Ok(())
            })
            .unwrap();
    }
#[test]
    pub(super) fn invalid_current_instruction_count_is_persisted_red_and_rejected() {
        let state = crate::test_support::app_state(database());
        let error = begin(
            &state,
            BeginGeneration {
                run_id: "run",
                provider_session_id: None,
                provider_id: Some("provider"),
                purpose: "reasoning",
                request_payload: b"request",
                envelope_payload: b"envelope",
                current_instruction_count: 0,
            },
        )
        .err()
        .expect("invalid envelope is rejected");
        assert!(error.contains("exactly one current instruction"));
        state
            .sqlite_readers
            .read(|connection| {
                let state: (String, String, u64) = connection
                    .query_row(
                        "SELECT health_status,status,current_instruction_count
                         FROM context_generations WHERE run_id='run'",
                        [],
                        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                    )
                    .map_err(database_error)?;
                assert_eq!(state, ("red".into(), "failed".into(), 0));
                Ok(())
            })
            .unwrap();
    }
#[test]
    pub(super) fn red_health_cannot_be_dispatched() {
        let state = crate::test_support::app_state(database());
        let generation = begin(
            &state,
            BeginGeneration {
                run_id: "run",
                provider_session_id: None,
                provider_id: Some("provider"),
                purpose: "context-build",
                request_payload: b"request",
                envelope_payload: b"envelope",
                current_instruction_count: 1,
            },
        )
        .unwrap();
        generation.set_health("red").unwrap();
        assert!(generation.dispatch().is_err());
        drop(generation);
        state
            .sqlite_readers
            .read(|connection| {
                let state: (String, String) = connection
                    .query_row(
                        "SELECT health_status,status FROM context_generations WHERE run_id='run'",
                        [],
                        |row| Ok((row.get(0)?, row.get(1)?)),
                    )
                    .map_err(database_error)?;
                assert_eq!(state, ("red".into(), "interrupted".into()));
                Ok(())
            })
            .unwrap();
    }
#[test]
    pub(super) fn manifest_records_selected_and_omitted_sources_without_content() {
        let state = crate::test_support::app_state(database());
        let generation = begin(
            &state,
            BeginGeneration {
                run_id: "run",
                provider_session_id: None,
                provider_id: Some("provider"),
                purpose: "reasoning",
                request_payload: b"request",
                envelope_payload: b"envelope",
                current_instruction_count: 1,
            },
        )
        .unwrap();
        generation
            .add_input(
                "fixture-selected",
                "assertion-1",
                3,
                &digest(b"selected"),
                "should",
                "base",
                true,
                None,
            )
            .unwrap();
        generation
            .add_input(
                "personal-pending",
                "message-2",
                1,
                &digest(b"omitted"),
                "may",
                "base",
                false,
                Some("budget-or-policy"),
            )
            .unwrap();
        generation.dispatch().unwrap();
        generation.complete().unwrap();
        state
            .sqlite_readers
            .read(|connection| {
                let rows: Vec<(String, bool, Option<String>)> = connection
                    .prepare(
                        "SELECT source_id,selected,omission_reason
                         FROM context_generation_inputs
                         WHERE source_kind IN ('fixture-selected','personal-pending') ORDER BY source_id",
                    )
                    .map_err(database_error)?
                    .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
                    .map_err(database_error)?
                    .collect::<Result<_, _>>()
                    .map_err(database_error)?;
                assert_eq!(
                    rows,
                    vec![
                        ("assertion-1".into(), true, None),
                        ("message-2".into(), false, Some("budget-or-policy".into()))
                    ]
                );
                Ok(())
            })
            .unwrap();
    }
#[test]
    pub(super) fn conservative_provider_budget_is_persisted_red() {
        let state = crate::test_support::app_state(database());
        let oversized = vec![b'x'; 96_001];
        assert!(begin(
            &state,
            BeginGeneration {
                run_id: "run",
                provider_session_id: None,
                provider_id: Some("provider"),
                purpose: "reasoning",
                request_payload: &oversized,
                envelope_payload: b"envelope",
                current_instruction_count: 1,
            },
        )
        .is_err());
        state
            .sqlite_readers
            .read(|connection| {
                let failure: String = connection
                    .query_row(
                        "SELECT failure_kind FROM context_generations WHERE run_id='run'",
                        [],
                        |row| row.get(0),
                    )
                    .map_err(database_error)?;
                assert_eq!(failure, "provider-input-budget");
                Ok(())
            })
            .unwrap();
    }
#[test]
    pub(super) fn policy_revision_change_rejects_a_late_completion() {
        let state = crate::test_support::app_state(database());
        let generation = begin(
            &state,
            BeginGeneration {
                run_id: "run",
                provider_session_id: None,
                provider_id: Some("provider"),
                purpose: "reasoning",
                request_payload: b"request",
                envelope_payload: b"envelope",
                current_instruction_count: 1,
            },
        )
        .unwrap();
        generation.dispatch().unwrap();
        state
            .sqlite_writer
            .write(|connection| {
                connection
                    .execute(
                        "UPDATE personal_scope SET policy_revision=policy_revision+1
                         WHERE id='primary'",
                        [],
                    )
                    .map_err(database_error)?;
                Ok(())
            })
            .unwrap();
        assert!(generation.complete().is_err());
    }
#[test]
    pub(super) fn scope_change_before_dispatch_rejects_the_generation_cas() {
        let connection = database();
        bind_scope(&connection);
        let state = crate::test_support::app_state(connection);
        let generation = begin(
            &state,
            BeginGeneration {
                run_id: "run",
                provider_session_id: None,
                provider_id: Some("provider"),
                purpose: "reasoning",
                request_payload: b"request",
                envelope_payload: b"envelope",
                current_instruction_count: 1,
            },
        )
        .unwrap();
        state
            .sqlite_writer
            .write(|connection| {
                connection
                    .execute(
                        "UPDATE context_scope_epochs SET epoch=epoch+1 WHERE scope_key='scope'",
                        [],
                    )
                    .map_err(database_error)?;
                Ok(())
            })
            .unwrap();
        assert!(generation.dispatch().is_err());
    }
