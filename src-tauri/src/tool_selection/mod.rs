//! Tool-selection ledger: a SQLite-backed catalog of L-Lang capabilities and external MCP tools,
//! hybrid retrieval with a local ML worker, conditional user-correction memory, and the three
//! conversation entry points. D4 adds external MCP Streamable HTTP sources; D5 (publishing SAAA
//! itself as an MCP server) and D6 (ranking learning) remain out of scope.

pub mod backends;
pub mod catalog;
pub mod contracts;
pub mod extraction;
pub mod feedback;
pub mod gateway;
pub mod gateway_schemas;
pub mod inference;
pub mod invocation;
pub mod invocation_task;
pub mod mcp;
pub mod mcp_server;
pub mod provider_extraction;
pub mod ranking;
pub mod references;
pub mod repository;
pub mod resolve;
pub mod retrieval;
pub mod rules;
pub mod schema;
pub mod service;
pub mod source_lookup;
pub mod worker;

mod tests;

pub use contracts::{
    InputKind, ObjectType, Operation, Phase, RequestContext, Scenario, SelectionMode,
    ToolSelectionConfig, ToolSelectionError, ToolSelectionErrorCode, ToolSelectionResult,
};
pub use service::ToolSelectionService;

/// Builds the live service from the parsed configuration. Discovery loads only local model files;
/// a missing manifest degrades inference but never falls back to another model. External MCP
/// sources are wired whenever an `mcpSourcesPath` is configured, independent of discovery mode.
pub(crate) fn build_service(
    writer: std::sync::Arc<crate::persistence::SqliteWriter>,
    config: &ToolSelectionConfig,
    capabilities: Option<std::sync::Arc<crate::generated_capabilities::service::CapabilityService>>,
) -> ToolSelectionService {
    use std::sync::Arc;

    // A crash can leave invocations in `running`; settle them before serving again.
    let _ = service::reconcile_interrupted_invocations(&writer);
    let extractor: Arc<dyn extraction::CorrectionExtractor> = if config.discovery_enabled() {
        Arc::new(provider_extraction::ConversationProviderExtractor::new(
            writer.clone(),
        ))
    } else {
        Arc::new(extraction::UnconfiguredExtractor)
    };
    let (embedding, reranker, threshold): (
        Arc<dyn inference::EmbeddingProvider>,
        Arc<dyn inference::RerankProvider>,
        f64,
    ) = if config.mode == SelectionMode::Mock {
        (
            Arc::new(inference::HashEmbedding::new(384)),
            Arc::new(inference::HashReranker::new()),
            f64::NEG_INFINITY,
        )
    } else if config.discovery_enabled() {
        let manifest_path = config
            .model_manifest_path
            .clone()
            .expect("discovery requires a manifest path");
        let manifest = inference::load_manifest(&manifest_path);
        match (manifest, config.python_path.clone()) {
            (Ok(manifest), Some(python_path)) => {
                let script = worker_script_path();
                let worker = Arc::new(worker::MlWorker::new(
                    python_path,
                    script,
                    manifest_path,
                    manifest.clone(),
                ));
                (
                    worker.clone() as Arc<dyn inference::EmbeddingProvider>,
                    worker as Arc<dyn inference::RerankProvider>,
                    manifest.no_match_threshold,
                )
            }
            _ => (
                Arc::new(inference::UnavailableEmbedding),
                Arc::new(inference::UnavailableReranker),
                f64::NEG_INFINITY,
            ),
        }
    } else {
        (
            Arc::new(inference::UnavailableEmbedding),
            Arc::new(inference::UnavailableReranker),
            f64::NEG_INFINITY,
        )
    };
    let (backend, manager): (
        Arc<dyn backends::ToolBackend>,
        Option<Arc<mcp::manager::McpManager>>,
    ) = if config.mode == SelectionMode::Mock {
        (Arc::new(backends::FixtureBackend::new()), None)
    } else {
        mcp::wiring::assemble(writer.clone(), config, capabilities, embedding.clone())
    };
    let mut service = ToolSelectionService::new(
        writer.clone(),
        embedding,
        reranker,
        extractor,
        backend,
        threshold,
    );
    // External MCP sources are usable through the same three entry points even when the local
    // discovery worker is not configured, so the definitions are exposed when either is present.
    service.set_discovery_configured(
        config.discovery_enabled() || config.mode == SelectionMode::Mock || manager.is_some(),
    );
    mcp::wiring::attach(&mut service, manager);
    if config.mode == SelectionMode::Mock {
        seed_mock_catalog_and_policy(&service, &writer);
    }
    service
}

/// Installs an explicit developer fixture only in `mode: "mock"`.  This path is opt-in through
/// the process configuration; it never runs for direct or real-model discovery and its backend
/// returns deterministic fixture data rather than contacting a capability or remote service.
fn seed_mock_catalog_and_policy(
    service: &ToolSelectionService,
    writer: &std::sync::Arc<crate::persistence::SqliteWriter>,
) {
    use catalog::{CatalogEntry, UsagePage};
    use rusqlite::OptionalExtension;
    use serde_json::json;

    let Ok(principal) = service::ensure_principal(writer) else {
        return;
    };
    let entry = |tool_id: &str, title: &str, purpose: &str| CatalogEntry {
        tool_id: tool_id.to_string(),
        backend_key: tool_id.to_string(),
        title: title.to_string(),
        purpose: purpose.to_string(),
        operations: vec!["search".to_string(), "read".to_string()],
        objects: vec!["development_fixture".to_string()],
        suitable: vec!["adaptive Tool-selection development tests".to_string()],
        unsuitable: vec!["production data or external side effects".to_string()],
        required_inputs: vec!["q".to_string()],
        input_schema: json!({
            "type": "object",
            "properties": { "q": { "type": "string" } },
            "required": ["q"],
            "additionalProperties": false
        }),
        output_schema: None,
        effect: "read",
        usage_pages: vec![UsagePage {
            section: "usage",
            page: 0,
            text:
                "Development-only deterministic fixture. It does not access user or external data."
                    .to_string(),
        }],
        backend_binding: json!({"kind":"fixture", "fixture": true}),
    };
    let entries = [
        entry(
            "adaptive-fixture-web",
            "Development fixture: web",
            "Deterministic stand-in for the rule-ranked web candidate.",
        ),
        entry(
            "adaptive-fixture-minutes",
            "Development fixture: minutes",
            "Deterministic stand-in for the learned preferred candidate.",
        ),
        entry(
            "adaptive-fixture-archive",
            "Development fixture: archive",
            "Deterministic stand-in for an alternate eligible candidate.",
        ),
    ];
    if service
        .ingest_catalog(&principal, "adaptive-development-fixture", &entries)
        .is_err()
    {
        return;
    }
    let _ = writer.write(|connection| {
        let candidates = vec![
            "adaptive-fixture-web-rev1".to_string(),
            "adaptive-fixture-minutes-rev1".to_string(),
            "adaptive-fixture-archive-rev1".to_string(),
        ];
        let existing_active: Option<String> = connection
            .query_row(
                "SELECT id FROM ai_artifacts WHERE domain='tool' AND scope_key='global' AND state='active' AND candidate_fingerprint=?1 LIMIT 1",
                [crate::adaptive_improvement::fingerprint_for(&candidates)],
                |row| row.get(0),
            )
            .optional()
            .map_err(|error| error.to_string())?;
        let artifact = if let Some(artifact) = existing_active {
            artifact
        } else {
            for (id, selected, successful) in [
                ("adaptive-fixture-outcome-web", "adaptive-fixture-web-rev1", false),
                (
                    "adaptive-fixture-outcome-minutes",
                    "adaptive-fixture-minutes-rev1",
                    true,
                ),
                (
                    "adaptive-fixture-outcome-archive",
                    "adaptive-fixture-archive-rev1",
                    false,
                ),
            ] {
                let exists: bool = connection
                    .query_row("SELECT EXISTS(SELECT 1 FROM ai_decisions WHERE id=?1)", [id], |row| row.get(0))
                    .map_err(|error| error.to_string())?;
                if !exists {
                    crate::adaptive_improvement::record_decision(
                        connection,
                        &crate::adaptive_improvement::DecisionObservation {
                            id: id.to_string(),
                            domain: crate::adaptive_improvement::Domain::Tool,
                            scope_key: "global".to_string(),
                            event_seq: 0,
                            policy_revision: 0,
                            candidate_fingerprint: crate::adaptive_improvement::fingerprint_for(&candidates),
                            eligible_candidates: candidates.clone(),
                            selected: selected.to_string(),
                            selection_mode: "rules".to_string(),
                            source_refs_json: json!({"fixture": "adaptive-development"}).to_string(),
                        },
                        0,
                    )?;
                    crate::adaptive_improvement::record_outcome(
                        connection,
                        id,
                        Some(successful),
                        None,
                        None,
                        None,
                        None,
                        None,
                        None,
                        "adaptive-development-fixture",
                        1,
                        0,
                    )?;
                }
            }
            let dataset = crate::adaptive_improvement::materialize_dirty(connection, 100, 0)?
                .ok_or_else(|| "mock fixture did not materialize its dataset".to_string())?;
            crate::adaptive_improvement::train_candidate_artifacts(connection, &dataset, 0)?
                .into_iter()
                .next()
                .ok_or_else(|| "mock fixture did not train a Tool artifact".to_string())?
        };
        let state: String = connection
            .query_row(
                "SELECT state FROM ai_artifacts WHERE id=?1",
                [&artifact],
                |row| row.get(0),
            )
            .map_err(|error| error.to_string())?;
        if state == "candidate" {
            crate::adaptive_improvement::apply_evaluation_gate(
                connection,
                &artifact,
                crate::adaptive_improvement::EvaluationGate {
                    examples: 200,
                    recipe_examples: 30,
                    independent_groups: 20,
                    protocol_errors: 0,
                    invalid_sources: 0,
                    scope_leaks: 0,
                    unknown_candidates: 0,
                    success_ci_lower: 0.01,
                    resource_improvement_ci_lower: Some(0.05),
                    other_resource_regression_upper: 0.0,
                },
            )?;
        }
        let state: String = connection
            .query_row(
                "SELECT state FROM ai_artifacts WHERE id=?1",
                [&artifact],
                |row| row.get(0),
            )
            .map_err(|error| error.to_string())?;
        if state == "shadow" {
            crate::adaptive_improvement::approve_shadow(connection, &artifact)?;
        }
        let state: String = connection
            .query_row(
                "SELECT state FROM ai_artifacts WHERE id=?1",
                [&artifact],
                |row| row.get(0),
            )
            .map_err(|error| error.to_string())?;
        if state == "eligible" {
            crate::adaptive_improvement::activate(connection, &artifact, 1, 0)?;
        }
        let mut settings = crate::persistence::load_role_routing_settings(connection)?;
        settings.adaptive_improvement.enabled = true;
        settings.adaptive_improvement.tool = true;
        connection
            .execute(
                "UPDATE settings_documents SET value_json=?1 WHERE namespace='routing.roles' AND key='default'",
                [serde_json::to_string(&settings).map_err(|error| error.to_string())?],
            )
            .map_err(|error| error.to_string())?;
        Ok(())
    });
}

/// Opens (or creates) a tool-selection database at `database_path` and builds the live service.
/// Used by the evaluation CLI and future management tooling. The seeded development fixture is
/// intentionally unavailable here: evaluators use `open_mock_service`, which supplies a blank
/// deterministic lane and records that it is mock rather than creating adaptive fixture history.
pub fn open_service(
    database_path: &std::path::Path,
    config: &ToolSelectionConfig,
) -> ToolSelectionResult<ToolSelectionService> {
    if config.mode == SelectionMode::Mock {
        return Err(ToolSelectionError::invalid());
    }
    let writer =
        crate::open_database_writer(database_path).map_err(|_| ToolSelectionError::storage())?;
    Ok(build_service(std::sync::Arc::new(writer), config, None))
}

/// Deterministic mock lane: hash embeddings and hash reranking. The evaluation CLI records
/// `lane=mock` so no one mistakes it for the real-model result.
pub fn open_mock_service(
    database_path: &std::path::Path,
) -> ToolSelectionResult<ToolSelectionService> {
    let writer =
        crate::open_database_writer(database_path).map_err(|_| ToolSelectionError::storage())?;
    let _ = service::reconcile_interrupted_invocations(&writer);
    let mut service = ToolSelectionService::new(
        std::sync::Arc::new(writer),
        std::sync::Arc::new(inference::HashEmbedding::new(384)),
        std::sync::Arc::new(inference::HashReranker::new()),
        std::sync::Arc::new(extraction::UnconfiguredExtractor),
        std::sync::Arc::new(backends::FixtureBackend::new()),
        f64::NEG_INFINITY,
    );
    service.set_discovery_configured(true);
    Ok(service)
}

fn worker_script_path() -> std::path::PathBuf {
    if let Some(path) = std::env::var_os("SAAA_TOOL_SELECTION_WORKER") {
        return std::path::PathBuf::from(path);
    }
    let base = option_env!("CARGO_MANIFEST_DIR").unwrap_or(".");
    std::path::Path::new(base)
        .join("..")
        .join("scripts")
        .join("tool-selection")
        .join("worker.py")
}

#[cfg(test)]
mod mock_tests {
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
}
