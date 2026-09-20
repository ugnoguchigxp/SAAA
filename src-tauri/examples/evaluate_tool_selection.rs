//! Evaluation pipeline for the tool-selection fixtures.
//!
//! The ranking itself runs in the library; this CLI only drives ingestion, search and JSON
//! output so `scripts/tool-selection/evaluate.py` never re-implements ranking.
//!
//! Usage:
//!   cargo run --example evaluate_tool_selection -- \
//!     --catalog <catalog.jsonl> --scenarios <scenarios.jsonl> --output <results.json> \
//!     [--mock | --manifest <manifest.json> --python <venv/bin/python>] [--database <sqlite>]

use std::path::PathBuf;

use saaa_lib::tool_selection::catalog::CatalogEntry;
use saaa_lib::tool_selection::inference;
use saaa_lib::tool_selection::{
    open_mock_service, open_service, InputKind, ObjectType, Operation, Phase, RequestContext,
    Scenario, SelectionMode, ToolSelectionConfig, ToolSelectionService,
};
use serde_json::{json, Value};

fn arg(args: &[String], name: &str) -> Option<String> {
    args.iter()
        .position(|value| value == name)
        .and_then(|index| args.get(index + 1).cloned())
}

fn catalog_entry(value: &Value) -> CatalogEntry {
    let strings = |key: &str| -> Vec<String> {
        value
            .get(key)
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default()
    };
    let usage = value
        .get("usage")
        .and_then(Value::as_array)
        .map(|pages| {
            pages
                .iter()
                .map(|page| saaa_lib::tool_selection::catalog::UsagePage {
                    section: match page.get("section").and_then(Value::as_str) {
                        Some("examples") => "examples",
                        Some("troubleshooting") => "troubleshooting",
                        _ => "usage",
                    },
                    page: page.get("page").and_then(Value::as_i64).unwrap_or(0),
                    text: page
                        .get("text")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                })
                .collect()
        })
        .unwrap_or_default();
    CatalogEntry {
        tool_id: value
            .get("toolId")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        backend_key: value
            .get("backendKey")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        title: value
            .get("title")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        purpose: value
            .get("purpose")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        operations: strings("operations"),
        objects: strings("objects"),
        suitable: strings("suitable"),
        unsuitable: strings("unsuitable"),
        required_inputs: strings("requiredInputs"),
        input_schema: value
            .get("inputSchema")
            .cloned()
            .unwrap_or_else(|| json!({ "type": "object" })),
        output_schema: value.get("outputSchema").cloned(),
        effect: "read",
        usage_pages: usage,
        backend_binding: value.get("backendBinding").cloned().unwrap_or(Value::Null),
    }
}

fn read_lines(path: &PathBuf) -> Vec<Value> {
    let text = std::fs::read_to_string(path).expect("read fixture");
    text.lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str(line).expect("fixture line is json"))
        .collect()
}

fn scenario_from(value: &Value) -> Scenario {
    Scenario {
        intent: value
            .get("intent")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        operation: Operation::parse(
            value
                .get("operation")
                .and_then(Value::as_str)
                .unwrap_or("unknown"),
        ),
        object_type: ObjectType::parse(
            value
                .get("objectType")
                .and_then(Value::as_str)
                .unwrap_or("unknown"),
        ),
        phase: Phase::Discover,
        input_kind: InputKind::Text,
    }
}

async fn run(service: &ToolSelectionService, scenarios: &[Value]) -> Vec<Value> {
    let mut results = Vec::new();
    for scenario in scenarios {
        let id = scenario
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let project = scenario.get("project").and_then(Value::as_str);
        let context = RequestContext::new("eval", "conversation_primary")
            .with_run(Some(id.to_string()))
            .with_project(project.map(str::to_string));
        let parsed = scenario_from(scenario);
        service.set_scenario(&context, parsed.clone());
        let response = match service.search(&context, &parsed.intent, 8).await {
            Ok(response) => response,
            Err(error) => {
                results.push(json!({
                    "id": id,
                    "status": "error",
                    "error": error.code.as_str(),
                    "predicted": [],
                }));
                continue;
            }
        };
        let decision_id = response.decision_id.clone();
        let candidates = service
            .decision_candidates(&decision_id)
            .unwrap_or_default();
        let predicted: Vec<String> = candidates
            .iter()
            .map(|candidate| candidate.tool_id.clone())
            .collect();
        let top_raw = candidates
            .iter()
            .filter_map(|candidate| candidate.raw_score)
            .fold(f64::NEG_INFINITY, f64::max);
        let gold: Vec<String> = scenario
            .get("gold")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default();
        results.push(json!({
            "id": id,
            "family": scenario.get("family").and_then(Value::as_str),
            "status": response.status.as_str(),
            "noMatch": scenario.get("noMatch").and_then(Value::as_bool).unwrap_or(false),
            "degraded": response.degraded,
            "gold": gold,
            "predicted": predicted,
            "topRaw": if top_raw.is_finite() { json!(top_raw) } else { Value::Null },
        }));
    }
    results
}

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().collect();
    let catalog_path = PathBuf::from(arg(&args, "--catalog").expect("--catalog"));
    let scenarios_path = PathBuf::from(arg(&args, "--scenarios").expect("--scenarios"));
    let output = PathBuf::from(arg(&args, "--output").expect("--output"));
    let database = PathBuf::from(arg(&args, "--database").unwrap_or_else(|| {
        std::env::temp_dir()
            .join(format!(
                "saaa-tool-selection-eval-{}.sqlite",
                std::process::id()
            ))
            .to_string_lossy()
            .to_string()
    }));
    if database.exists() {
        let _ = std::fs::remove_file(&database);
    }

    let manifest = arg(&args, "--manifest");
    let python = arg(&args, "--python");
    let (service, lane, model_hash) = match (manifest, python) {
        (Some(manifest), Some(python)) => {
            let mut config = ToolSelectionConfig::direct();
            config.mode = SelectionMode::Discovery;
            config.model_manifest_path = Some(PathBuf::from(&manifest));
            config.python_path = Some(PathBuf::from(&python));
            let service = open_service(&database, &config).expect("open service");
            let hash = inference::load_manifest(std::path::Path::new(&manifest))
                .map(|manifest| manifest.embedding_hash)
                .unwrap_or_else(|_| "unknown".to_string());
            (service, "live", hash)
        }
        _ => {
            let service = open_mock_service(&database).expect("open mock service");
            (service, "mock", "test-hash-384".to_string())
        }
    };
    let catalog_rows = read_lines(&catalog_path);
    let entries: Vec<CatalogEntry> = catalog_rows.iter().map(catalog_entry).collect();
    let ingested = service
        .ingest_catalog("eval", "llang", &entries)
        .expect("ingest catalog");
    let indexed = service
        .index_embeddings("eval", None)
        .await
        .expect("index embeddings");
    let scenarios = read_lines(&scenarios_path);
    let results = run(&service, &scenarios).await;

    let payload = json!({
        "formatVersion": 1,
        "lane": lane,
        "embeddingModelHash": model_hash,
        "catalogSize": ingested,
        "indexed": indexed,
        "results": results,
    });
    if let Some(parent) = output.parent() {
        std::fs::create_dir_all(parent).ok();
    }
    std::fs::write(
        &output,
        serde_json::to_vec_pretty(&payload).expect("serialize"),
    )
    .expect("write output");
    println!("wrote {}", output.display());
}
