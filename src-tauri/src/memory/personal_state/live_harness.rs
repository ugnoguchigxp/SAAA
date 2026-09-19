//! Feature-gated synthetic runner: real source persistence, worker and projection.
use super::*;
use crate::{database_error, persistence::sqlite::SqliteWriter};
use serde_json::{json, Value};
use std::sync::Arc;
pub async fn run_json(raw: &str) -> Result<String, String> {
    if raw.len() > 262144 {
        return Err("harness-input-budget".into());
    }
    let input: Value = serde_json::from_str(raw).map_err(|_| "harness-input-schema")?;
    if input["schemaVersion"] != 1 || input["resetState"] != true || input.get("gold").is_some() {
        return Err("harness-input-schema".into());
    }
    let c = rusqlite::Connection::open_in_memory().map_err(database_error)?;
    crate::persistence::schema::initialize_database(&c).map_err(database_error)?;
    // A stable synthetic owner avoids creating one Keychain record per scenario.
    c.execute(
        "UPDATE personal_scope SET principal='personal-state-synthetic-eval'",
        [],
    )
    .map_err(database_error)?;
    let sources = input["sources"]
        .as_array()
        .filter(|s| !s.is_empty() && s.len() <= 64)
        .ok_or("harness-sources")?;
    for s in sources {
        let id = s["id"].as_str().ok_or("harness-source")?;
        crate::validate_identifier(id, "synthetic source")?;
        let text = s["text"].as_str().ok_or("harness-source")?;
        if s["version"] != 1 {
            return Err("harness-source-version".into());
        }
        c.execute(
            "INSERT INTO conversation_messages VALUES(?1,?2,'user',?3,'1')",
            rusqlite::params![id, crate::PRIMARY_CONVERSATION_ID, text],
        )
        .map_err(database_error)?;
    }
    let writer = Arc::new(SqliteWriter::from_connection(c));
    let a = managed::Adapter::configured(writer.clone()).await?;
    let deployment = &input["deployment"];
    if deployment["model"] != a.certification.model
        || deployment["release"] != a.certification.release
        || deployment["runtime"] != a.certification.runtime
        || deployment["tokenizerDigest"] != a.certification.tokenizer_digest
        || deployment["maxInputTokens"] != a.certification.max_input_tokens
        || deployment["maxInputBytes"] != a.certification.max_bytes
    {
        return Err("harness-deployment-binding".into());
    }
    let started = std::time::Instant::now();
    let mut violations = Vec::new();
    for _ in 0..128 {
        match worker::tick(&writer, &a, true).await {
            Ok(true) => {}
            Ok(false) => break,
            Err(_) => {
                violations.push("extraction_failed");
                break;
            }
        }
        if started.elapsed().as_secs() >= 25 {
            violations.push("harness-timeout");
            break;
        }
    }
    let mut result=writer.read_serialized(|c|{
        let ledger=store::load(c)?;let mut items=Vec::new();
        for assertion in ledger.assertions.values(){
            if assertion.kind.is_world(){continue;}
            let status=ledger.status(&assertion.id,now());
            if !matches!(status,saaa_personal_state_core::Status::Active|saaa_personal_state_core::Status::Candidate|saaa_personal_state_core::Status::Disputed){continue;}
            let value:String=c.query_row("SELECT value_json FROM personal_payloads WHERE id=?1",[&assertion.payload_ref],|r|r.get(0)).map_err(database_error)?;
            let value:Value=decode(value)?;
            for source in &assertion.evidence{items.push(json!({"kind":assertion.kind,"target":assertion.access.task_request.as_deref().unwrap_or("primary"),"value":value.as_str().map(str::to_string).unwrap_or_else(||value.to_string()),"status":status,"source":source}));}
        }
        let mut q=c.prepare("SELECT s.message_id FROM personal_sources s JOIN personal_jobs j ON j.source_sequence=s.sequence WHERE j.status='completed' AND s.available=1").map_err(database_error)?;
        let covered=q.query_map([],|r|r.get::<_,String>(0)).map_err(database_error)?.collect::<Result<Vec<_>,_>>().map_err(database_error)?;
        Ok(json!({"scenarioId":input["scenarioId"],"repetition":input["repetition"],"checkpoint":input["checkpoint"],"items":items,"coveredSources":covered,"elapsedMs":started.elapsed().as_millis() as u64,"violations":violations}))
    })?;
    if a.cleanup().await.is_err()
        || writer.read_serialized(|c| {
            c.query_row(
                "SELECT EXISTS(SELECT 1 FROM personal_remote_operations WHERE state!='cleaned')",
                [],
                |r| r.get::<_, bool>(0),
            )
            .map_err(database_error)
        })?
    {
        result["violations"]
            .as_array_mut()
            .ok_or("harness-result")?
            .push(json!("remote_cleanup_pending"));
    }
    drop(a);
    product_binding::shutdown().await;
    encode(&result)
}
