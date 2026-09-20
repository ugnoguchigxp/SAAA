//! One-transaction publication of a generated capability into tool_selection (plan 12.5, C07).
//!
//! No model, process, or embedding work happens while the writer lock is held.

use rusqlite::Connection;
use serde_json::{json, Value};

use super::contracts::WasmContract;
use super::errors::*;
use super::generation::contracts::GenerationStatus;
use super::generation::repository as generation_repository;
use super::repository::{self, Activation};
use crate::now_iso;
use crate::tool_selection::catalog::{self, CatalogEntry, UsagePage};
use crate::tool_selection::catalog::{hex_sha256, canonical_json};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PublishStep {
    Activate,
    Catalog,
    Grant,
    Job,
}

pub struct PublishRequest<'a> {
    pub principal_id: &'a str,
    pub revision_id: &'a str,
    pub expected_epoch: i64,
    pub job_id: Option<&'a str>,
    pub grant_on_create: bool,
    pub fail_at: Option<PublishStep>,
}

pub fn source_id(principal_id: &str) -> String {
    format!("llang-generated:{principal_id}")
}

pub fn tool_id(source_id: &str, capability_id: &str) -> String {
    let canonical = canonical_json(&json!([source_id, capability_id]));
    format!("llgt_{}", hex_sha256(canonical.as_bytes()))
}

pub fn published_revision_id(
    tool_id: &str,
    generated_revision_id: &str,
    contract_hash: &str,
    catalog_epoch: i64,
) -> String {
    let canonical = canonical_json(&json!([
        tool_id,
        generated_revision_id,
        contract_hash,
        catalog_epoch
    ]));
    format!("llgr_{}", hex_sha256(canonical.as_bytes()))
}

/// Activates the generated revision and publishes it to tool_selection in the caller's
/// transaction. Any failure rolls the whole transaction back.
pub fn activate_and_publish(
    connection: &Connection,
    request: PublishRequest<'_>,
) -> CapabilityResult<Activation> {
    fail(request.fail_at, PublishStep::Activate)?;
    let activation = repository::activate(
        connection,
        request.revision_id,
        request.expected_epoch,
        &now_iso(),
    )?;
    let revision = repository::revision_by_id(connection, request.revision_id)?;
    let source = source_id(request.principal_id);
    let tool = tool_id(&source, &activation.capability_id);
    let published_id = published_revision_id(
        &tool,
        &revision.id,
        &revision.contract_hash,
        activation.catalog_epoch,
    );
    let entry = catalog_entry(&tool, &revision)?;
    fail(request.fail_at, PublishStep::Catalog)?;
    catalog::register_revision(
        connection,
        request.principal_id,
        &source,
        &entry,
        &published_id,
        unix_ms(),
    )
    .map_err(|_| {
        CapabilityError::new(
            CapabilityErrorCode::StorageError,
            "tool catalog publication failed",
        )
    })?;
    if request.grant_on_create {
        fail(request.fail_at, PublishStep::Grant)?;
        catalog::grant_user(connection, request.principal_id, &tool).map_err(|_| {
            CapabilityError::new(
                CapabilityErrorCode::StorageError,
                "initial grant failed",
            )
        })?;
    }
    if let Some(job_id) = request.job_id {
        fail(request.fail_at, PublishStep::Job)?;
        let moved = generation_repository::finish(
            connection,
            job_id,
            GenerationStatus::AwaitingActivation,
            GenerationStatus::Active,
            None,
            None,
            unix_ms(),
        )?;
        if !moved {
            return error(
                CapabilityErrorCode::Conflict,
                "generation job was not awaiting activation",
            );
        }
    }
    Ok(activation)
}

pub fn unpublish_tool_for_capability(
    connection: &Connection,
    principal_id: &str,
    capability_id: &str,
) -> CapabilityResult<()> {
    let source = source_id(principal_id);
    let tool = tool_id(&source, capability_id);
    catalog::unpublish_tool(connection, &tool).map_err(|_| {
        CapabilityError::new(
            CapabilityErrorCode::StorageError,
            "tool catalog unpublish failed",
        )
    })
}

fn catalog_entry(tool: &str, revision: &repository::RevisionRow) -> CapabilityResult<CatalogEntry> {
    let contract: WasmContract = serde_json::from_str(&revision.contract_json).map_err(|_| {
        CapabilityError::new(
            CapabilityErrorCode::IntegrityError,
            "revision contract is not valid JSON",
        )
    })?;
    let fields: Vec<String> = contract.fields.iter().map(|field| field.name.clone()).collect();
    let schema = json!({
        "type": "object",
        "properties": fields.iter().cloned().map(|name| (name, json!({"type":"boolean"}))).collect::<serde_json::Map<String, Value>>(),
        "required": fields,
        "additionalProperties": false
    });
    let binding = json!({
        "capabilityId": revision.capability_id,
        "revisionId": revision.id,
        "packageHash": revision.package_hash,
        "contractHash": revision.contract_hash,
        "catalogEpoch": 0,
        "inputFields": fields
    });
    Ok(CatalogEntry {
        tool_id: tool.to_string(),
        backend_key: tool.to_string(),
        title: revision.capability_id.clone(),
        purpose: "Generated L-Lang predicate".to_string(),
        operations: vec!["evaluate".to_string()],
        objects: vec!["predicate".to_string()],
        suitable: vec!["boolean predicate evaluation".to_string()],
        unsuitable: vec!["side effects".to_string()],
        required_inputs: fields,
        input_schema: schema,
        output_schema: Some(json!({"type":"boolean"})),
        effect: "none",
        usage_pages: vec![UsagePage {
            section: "overview",
            page: 1,
            text: format!("Generated capability {}", revision.capability_id),
        }],
        backend_binding: binding,
    })
}

fn unix_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or(0)
}

fn fail(at: Option<PublishStep>, step: PublishStep) -> CapabilityResult<()> {
    if at == Some(step) {
        return error(
            CapabilityErrorCode::StorageError,
            "injected publication failure",
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::generated_capabilities::lifecycle;
    use crate::generated_capabilities::tests::{TestEnv, ACCEPTANCE_A, CANDIDATE_A};
    use crate::tool_selection::repository as ts_repo;

    async fn verified(env: &TestEnv) -> (String, i64) {
        let revision = env.import(CANDIDATE_A, ACCEPTANCE_A).await.expect("import");
        let summary = env
            .verify(&revision, ACCEPTANCE_A)
            .await
            .expect("verify");
        assert!(summary.passed);
        let epoch = env
            .service
            .catalog_epoch(&revision.capability_id)
            .expect("epoch");
        (revision.revision_id, epoch)
    }

    #[tokio::test]
    async fn rw_04_catalog_failure_rolls_back_activation() {
        let env = TestEnv::start(true);
        let (revision_id, epoch) = verified(&env).await;
        let principal = crate::tool_selection::service::ensure_principal(&env.writer)
            .expect("principal");
        let result = lifecycle::transaction(&env.writer, |transaction| {
            activate_and_publish(
                transaction,
                PublishRequest {
                    principal_id: &principal,
                    revision_id: &revision_id,
                    expected_epoch: epoch,
                    job_id: None,
                    grant_on_create: false,
                    fail_at: Some(PublishStep::Catalog),
                },
            )
        });
        assert!(result.is_err());
        assert_ne!(
            env.revision_state(&revision_id),
            repository::RevisionState::Active
        );
        let tools: i64 = env
            .writer
            .read_serialized(|connection| {
                connection
                    .query_row("SELECT COUNT(*) FROM tool_selection_catalog", [], |row| {
                        row.get(0)
                    })
                    .map_err(crate::database_error)
            })
            .expect("count");
        assert_eq!(tools, 0);
    }

    #[tokio::test]
    async fn rw_04_publish_registers_one_catalog_revision() {
        let env = TestEnv::start(true);
        let (revision_id, epoch) = verified(&env).await;
        let principal = crate::tool_selection::service::ensure_principal(&env.writer)
            .expect("principal");
        lifecycle::transaction(&env.writer, |transaction| {
            activate_and_publish(
                transaction,
                PublishRequest {
                    principal_id: &principal,
                    revision_id: &revision_id,
                    expected_epoch: epoch,
                    job_id: None,
                    grant_on_create: true,
                    fail_at: None,
                },
            )
        })
        .expect("publish");
        assert_eq!(
            env.revision_state(&revision_id),
            repository::RevisionState::Active
        );
        let grants: i64 = env
            .writer
            .read_serialized(|connection| {
                let epochs = ts_repo::epochs(connection).map_err(crate::database_error)?;
                assert!(epochs.catalog >= 1);
                assert!(epochs.acl >= 1);
                connection
                    .query_row("SELECT COUNT(*) FROM tool_selection_grants", [], |row| {
                        row.get(0)
                    })
                    .map_err(crate::database_error)
            })
            .expect("grants");
        assert_eq!(grants, 1);
    }
}
