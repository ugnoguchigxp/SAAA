//! Service-side support for MCP bindings: source display identity, the large-result store and
//! continuation lookup, and admission-error mapping. Kept out of `service.rs` so the base service
//! file stays close to its pre-D4 size.

use rusqlite::TransactionBehavior;
use serde_json::Value;

use super::super::backends::TechnicalStatus;
use super::super::contracts::{
    RequestContext, ToolSelectionError, ToolSelectionErrorCode, ToolSelectionResult,
};
use super::super::repository::{self, RevisionRow};
use super::results::{self, StoreOutcome};
use super::session::CallError;
use super::MCP_RESULT_MAX_BYTES;
use crate::persistence::SqliteWriter;
use crate::tool_selection::service::ResultPageResponse;

/// Maps an admission or source-eligibility failure to the existing gateway error contract.
pub fn map_call_error(error: CallError) -> ToolSelectionError {
    match error {
        CallError::Unavailable("source-changed" | "source-stale" | "source-not-synced") => {
            ToolSelectionError::stale()
        }
        CallError::Unavailable(_) => ToolSelectionError::unavailable(),
        CallError::Busy => ToolSelectionError::capacity(),
        CallError::CancelledBeforeSend => ToolSelectionError::new(
            ToolSelectionErrorCode::Cancelled,
            "The tool call was cancelled.",
        ),
        CallError::Closed
        | CallError::SessionExpired
        | CallError::Protocol(_)
        | CallError::Unknown(_)
        | CallError::RpcError => ToolSelectionError::unavailable(),
    }
}

/// Display identity for a tool: the source id plus a label that distinguishes same-named tools from
/// different connection targets.
pub fn source_display(writer: &SqliteWriter, tool_id: &str) -> (String, String) {
    let tool_id = tool_id.to_string();
    let resolved = writer
        .read_serialized(move |connection| {
            let tool =
                repository::tool_by_id(connection, &tool_id).map_err(|error| error.to_string())?;
            let kind = tool.as_ref().and_then(|tool| {
                repository::source_kind(connection, &tool.source_id)
                    .ok()
                    .flatten()
            });
            Ok((tool.map(|tool| tool.source_id).unwrap_or_default(), kind))
        })
        .unwrap_or_default();
    let (source_id, kind) = resolved;
    let label = match kind.as_deref() {
        Some("mcp_http") => source_id.clone(),
        _ => "l-lang".to_string(),
    };
    (source_id, label)
}

#[allow(clippy::too_many_arguments)]
pub fn store_large_result(
    writer: &SqliteWriter,
    invocation_id: &str,
    context: &RequestContext,
    tool_id: &str,
    revision_id: &str,
    schema_hash: &str,
    acl_epoch: i64,
    canonical: &str,
) -> ToolSelectionResult<StoreOutcome> {
    let invocation_id = invocation_id.to_string();
    let principal = context.principal_id.clone();
    let conversation = context.conversation_id.clone();
    let scope_key = context.scope_key();
    let tool_id = tool_id.to_string();
    let revision_id = revision_id.to_string();
    let schema_hash = schema_hash.to_string();
    let canonical = canonical.to_string();
    writer
        .write(move |connection| {
            let transaction = connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(crate::database_error)?;
            let outcome = results::store_result(
                &transaction,
                &invocation_id,
                &principal,
                &conversation,
                &scope_key,
                &tool_id,
                &revision_id,
                &schema_hash,
                acl_epoch,
                &canonical,
            )
            .map_err(|error| error.code.as_str().to_string())?;
            transaction.commit().map_err(crate::database_error)?;
            Ok(outcome)
        })
        .map_err(|_| ToolSelectionError::storage())
}

/// True when a canonical result is too big for the inline envelope and needs continuation storage.
pub fn exceeds_inline(byte_count: usize) -> bool {
    byte_count > super::super::contracts::BACKEND_RESULT_MAX_BYTES
}

/// The technical outcome after output-schema validation and large-result handling.
pub struct FinalizedOutcome {
    pub status: TechnicalStatus,
    pub error_code: Option<&'static str>,
    pub result: Option<Value>,
    pub result_ref: Option<String>,
    pub byte_count: Option<i64>,
    pub page_count: Option<i64>,
    pub result_availability: Option<&'static str>,
}

/// Applies output-schema validation and the 16 KiB / 1 MiB result rules. A stored result keeps the
/// technical status `succeeded`; a result that cannot be stored is marked `unavailable` without
/// changing whether the remote call itself succeeded.
#[allow(clippy::too_many_arguments)]
pub fn finalize_outcome(
    writer: &SqliteWriter,
    invocation_id: &str,
    context: &RequestContext,
    tool_id: &str,
    revision: &RevisionRow,
    acl_epoch: i64,
    binding_kind: &str,
    outcome_status: TechnicalStatus,
    outcome_error: Option<&'static str>,
    outcome_result: Option<Value>,
) -> FinalizedOutcome {
    let mut status = outcome_status;
    let mut error_code = outcome_error;
    let mut result = outcome_result;
    let mut result_ref = None;
    let mut byte_count = None;
    let mut page_count = None;
    let mut result_availability = None;

    if status == TechnicalStatus::Succeeded {
        if let (Some(schema), Some(structured)) = (
            revision.output_schema.as_ref(),
            result
                .as_ref()
                .and_then(|value| value.get("structuredContent")),
        ) {
            let valid = jsonschema::validator_for(schema)
                .map(|validator| validator.is_valid(structured))
                .unwrap_or(false);
            if !valid {
                status = TechnicalStatus::Failed;
                error_code = Some("remote-result-invalid");
                result = None;
            }
        }
    }

    if status == TechnicalStatus::Succeeded {
        if let Some(value) = result.take() {
            let canonical = super::descriptors::canonical_json_string(&value);
            let bytes = canonical.len();
            if exceeds_inline(bytes) {
                if binding_kind == "mcp_http" {
                    if exceeds_storage(bytes) {
                        result_availability = Some("unavailable");
                        error_code = Some("result-size-limit");
                    } else {
                        match store_large_result(
                            writer,
                            invocation_id,
                            context,
                            tool_id,
                            &revision.id,
                            &revision.schema_hash,
                            acl_epoch,
                            &canonical,
                        ) {
                            Ok(StoreOutcome::Stored {
                                result_ref: reference_id,
                                byte_count: stored_bytes,
                                page_count: pages,
                            }) => {
                                result_ref = Some(reference_id);
                                byte_count = Some(stored_bytes);
                                page_count = Some(pages);
                                result_availability = Some("stored");
                            }
                            Ok(StoreOutcome::StorageLimit) => {
                                result_availability = Some("unavailable");
                                error_code = Some("result-storage-limit");
                            }
                            Ok(StoreOutcome::SizeLimit) => {
                                result_availability = Some("unavailable");
                                error_code = Some("result-size-limit");
                            }
                            // A storage error while persisting the result leaves the remote
                            // success intact but the content unavailable.
                            Err(_) => {
                                result_availability = Some("unavailable");
                                error_code = Some("result-storage-limit");
                            }
                        }
                    }
                } else {
                    // Existing L-Lang behavior is preserved: the result is bounded at 16 KiB.
                    status = TechnicalStatus::Failed;
                    error_code = Some("output-limit");
                }
            } else {
                result = Some(value);
                result_availability = Some("inline");
            }
        } else {
            result_availability = Some("inline");
        }
    } else if let Some(value) = &result {
        let bytes = super::descriptors::canonical_json_string(value).len();
        if bytes > MCP_RESULT_MAX_BYTES {
            result = None;
        }
        result_availability = Some("inline");
    } else {
        result_availability = Some("unavailable");
    }

    FinalizedOutcome {
        status,
        error_code,
        result,
        result_ref,
        byte_count,
        page_count,
        result_availability,
    }
}

/// True when a canonical result is too big to store at all.
pub fn exceeds_storage(byte_count: usize) -> bool {
    byte_count > MCP_RESULT_MAX_BYTES
}

/// Resolves one continuation page. Ownership, scope, TTL and the current ACL are re-checked on
/// every read; a revoked or foreign reference is refused.
pub fn describe_result(
    writer: &SqliteWriter,
    context: &RequestContext,
    result_ref: &str,
    page: i64,
) -> ToolSelectionResult<ResultPageResponse> {
    if result_ref.is_empty() {
        return Err(ToolSelectionError::invalid());
    }
    let principal = context.principal_id.clone();
    let project = context.project_id.clone();
    let scope_key = context.scope_key();
    let result_ref = result_ref.to_string();
    writer
        .read_serialized(move |connection| {
            results::read_page(
                connection,
                &principal,
                project.as_deref(),
                &scope_key,
                &result_ref,
                page,
            )
            .map(|page| ResultPageResponse {
                result_ref: result_ref.clone(),
                page: page.page,
                page_count: page.page_count,
                text: page.text,
            })
            .map_err(|error| error.code.as_str().to_string())
        })
        .map_err(|code| match code.as_str() {
            "not-found" => ToolSelectionError::not_found(),
            "not-authorized" => ToolSelectionError::unauthorized(),
            "invalid-input" => ToolSelectionError::invalid(),
            _ => ToolSelectionError::storage(),
        })
}
