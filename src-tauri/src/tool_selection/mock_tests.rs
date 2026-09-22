#![cfg(test)]
use super::*;
use rusqlite::Connection;
use serde_json::json;

#[test]
fn mock_mode_seeds_only_the_explicit_development_catalog() {
    let connection = Connection::open_in_memory().expect("in-memory database");
    crate::persistence::schema::initialize_database(&connection).expect("schema");
    let writer = std::sync::Arc::new(crate::persistence::SqliteWriter::from_connection(
        connection,
    ));
    let config = ToolSelectionConfig {
        mode: SelectionMode::Mock,
        python_path: None,
        model_manifest_path: None,
        mcp_sources_path: None,
        extraction: contracts::ExtractionSetting::ConfiguredConversationProvider,
        diagnostic: None,
    };
    let service = build_service(writer.clone(), &config, None);
    assert!(service.discovery_configured());
    let count: i64 = writer
        .read_serialized(|connection| {
            connection
                .query_row(
                    "SELECT COUNT(*) FROM tool_selection_catalog WHERE source_id='adaptive-development-fixture'",
                    [],
                    |row| row.get(0),
                )
                .map_err(|error| error.to_string())
    })
    .expect("fixture catalog count");
    assert_eq!(count, 3);
    let selected = writer
        .read_serialized(|connection| {
            crate::adaptive_improvement::choose(
                connection,
                crate::adaptive_improvement::Domain::Tool,
                "global",
                &[
                    "adaptive-fixture-web-rev1".to_string(),
                    "adaptive-fixture-minutes-rev1".to_string(),
                    "adaptive-fixture-archive-rev1".to_string(),
                ],
                "adaptive-fixture-web-rev1",
                1,
            )
        })
        .expect("fixture policy selection");
    assert_eq!(
        selected,
        ("adaptive-fixture-minutes-rev1".to_string(), "adaptive", 1)
    );
    let (examples, scores): (i64, String) = writer
        .read_serialized(|connection| {
            let examples = connection
                .query_row(
                    "SELECT COUNT(*) FROM ai_examples WHERE decision_id LIKE 'adaptive-fixture-outcome-%'",
                    [],
                    |row| row.get(0),
                )
                .map_err(|error| error.to_string())?;
            let scores = connection
                .query_row(
                    "SELECT scores_json FROM ai_artifacts WHERE domain='tool' AND scope_key='global' AND state='active'",
                    [],
                    |row| row.get(0),
                )
                .map_err(|error| error.to_string())?;
            Ok((examples, scores))
        })
        .expect("fixture training evidence");
    assert_eq!(examples, 3);
    assert_eq!(
        scores,
        r#"{"adaptive-fixture-archive-rev1":0.0,"adaptive-fixture-minutes-rev1":1.0,"adaptive-fixture-web-rev1":0.0}"#
    );
}

#[test]
fn direct_mode_never_seeds_development_fixture_tools() {
    let connection = Connection::open_in_memory().expect("in-memory database");
    crate::persistence::schema::initialize_database(&connection).expect("schema");
    let writer = std::sync::Arc::new(crate::persistence::SqliteWriter::from_connection(
        connection,
    ));
    let _service = build_service(writer.clone(), &ToolSelectionConfig::direct(), None);
    let count: i64 = writer
        .read_serialized(|connection| {
            connection
                .query_row(
                    "SELECT COUNT(*) FROM tool_selection_catalog WHERE source_id='adaptive-development-fixture'",
                    [],
                    |row| row.get(0),
                )
                .map_err(|error| error.to_string())
        })
        .expect("fixture catalog count");
    assert_eq!(count, 0);
}

#[test]
fn generic_service_opener_refuses_the_seeded_mock_configuration() {
    let directory = tempfile::tempdir().expect("temporary database directory");
    let config = ToolSelectionConfig {
        mode: SelectionMode::Mock,
        python_path: None,
        model_manifest_path: None,
        mcp_sources_path: None,
        extraction: contracts::ExtractionSetting::ConfiguredConversationProvider,
        diagnostic: None,
    };
    let error = match open_service(&directory.path().join("evaluation.sqlite3"), &config) {
        Ok(_) => panic!("generic service cannot seed the adaptive fixture"),
        Err(error) => error,
    };
    assert_eq!(error.code, contracts::ToolSelectionErrorCode::InvalidInput);
}

#[tokio::test]
async fn mock_mode_uses_trained_fixture_artifact_for_search_and_invoke() {
    let connection = Connection::open_in_memory().expect("in-memory database");
    crate::persistence::schema::initialize_database(&connection).expect("schema");
    let writer = std::sync::Arc::new(crate::persistence::SqliteWriter::from_connection(
        connection,
    ));
    let config = ToolSelectionConfig {
        mode: SelectionMode::Mock,
        python_path: None,
        model_manifest_path: None,
        mcp_sources_path: None,
        extraction: contracts::ExtractionSetting::ConfiguredConversationProvider,
        diagnostic: None,
    };
    let service = build_service(writer.clone(), &config, None);
    let principal = service::ensure_principal(&writer).expect("fixture principal");
    writer
        .write(|connection| {
            connection
                .execute(
                    "INSERT INTO conversations(id,title,task_mode,created_at,updated_at) VALUES('fixture-conversation','fixture','conversation','2026-01-01T00:00:00Z','2026-01-01T00:00:00Z')",
                    [],
                )
                .map_err(|error| error.to_string())?;
            Ok(())
        })
        .expect("fixture conversation");
    let context = RequestContext::new(&principal, "fixture-conversation");
    service.set_scenario(
        &context,
        Scenario {
            intent: "development fixture".to_string(),
            operation: Operation::Search,
            object_type: ObjectType::DecisionRecord,
            phase: Phase::Discover,
            input_kind: InputKind::Text,
        },
    );
    let response = service
        .search(&context, "development fixture", 3)
        .await
        .expect("mock search");
    let selected = response.candidates.first().expect("selected fixture Tool");
    assert_eq!(selected.tool_id, "adaptive-fixture-minutes");
    let execution_ref = service
        .describe(&context, &selected.reference, "usage", None)
        .expect("describe selected fixture Tool")
        .execution_ref
        .expect("execution reference");
    let invocation = service
        .invoke(
            &context,
            &execution_ref,
            &json!({"q": "fixture"}),
            &crate::RunCancellation::default(),
        )
        .await
        .expect("mock invoke");
    assert_eq!(invocation.status, backends::TechnicalStatus::Succeeded);
    let technical_success: i64 = writer
        .read_serialized(|connection| {
            connection
                .query_row(
                    "SELECT technical_success FROM ai_outcomes ORDER BY created_at_ms DESC LIMIT 1",
                    [],
                    |row| row.get(0),
                )
                .map_err(|error| error.to_string())
        })
        .expect("mock Tool outcome");
    assert_eq!(technical_success, 1);
}
