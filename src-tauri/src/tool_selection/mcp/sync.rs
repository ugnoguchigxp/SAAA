//! Atomic full-snapshot sync of one external MCP source.
//!
//! All pages are fetched into memory first, validated, and only then published in a single SQLite
//! transaction. A failure at any step leaves the previous snapshot and catalog epoch untouched;
//! a failure is never interpreted as "remove every tool". Publishing re-checks the config
//! generation inside the transaction, so a settings change during the fetch wins.

use rusqlite::TransactionBehavior;
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use super::super::contracts::now_ms;
use super::super::repository;
use super::descriptors::{self, NormalizedTool};
use super::repository as mcp_repository;
use super::session::{CallError, McpSessionPool};
use super::{
    MCP_LIST_DEADLINE, MCP_LIST_PAGES_MAX, MCP_LIST_PAGE_MAX_BYTES, MCP_LIST_TOTAL_MAX_BYTES,
    MCP_TOOLS_PER_PROFILE_MAX, MCP_TOOLS_PER_SOURCE_MAX,
};
use crate::persistence::SqliteWriter;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SyncError {
    pub code: &'static str,
}

impl SyncError {
    fn new(code: &'static str) -> Self {
        Self { code }
    }
}

impl From<CallError> for SyncError {
    fn from(error: CallError) -> Self {
        Self { code: error.code() }
    }
}

#[derive(Clone, Debug, Default)]
pub struct SyncOutcome {
    pub published: usize,
    pub added: usize,
    pub changed: usize,
    pub disabled: usize,
    pub epoch_bumped: bool,
    pub new_revision_ids: Vec<String>,
    pub changed_tool_ids: Vec<String>,
}

struct ExistingTool {
    pub(super) enabled: bool,
    pub(super) current_revision_id: Option<String>,
}

/// Runs a full sync and publishes it atomically. `owner_principal` is the host-confirmed local
/// principal; it is never read from the source document.
pub async fn sync_source(
    writer: &Arc<SqliteWriter>,
    pool: &McpSessionPool,
    source: &super::config::McpSourceSpec,
    config_generation: i64,
    owner_principal: &str,
    endpoint_hash: &str,
) -> Result<SyncOutcome, SyncError> {
    let raw = fetch_all(pool, source, config_generation).await?;
    let normalized = prepare(&source.id, endpoint_hash, raw)?;
    publish(
        writer,
        &source.id,
        config_generation,
        owner_principal,
        endpoint_hash,
        normalized,
    )
}

async fn fetch_all(
    pool: &McpSessionPool,
    source: &super::config::McpSourceSpec,
    config_generation: i64,
) -> Result<Vec<Value>, SyncError> {
    let session = pool
        .session_for(source, config_generation)
        .await
        .map_err(SyncError::from)?;
    let deadline = tokio::time::Instant::now() + MCP_LIST_DEADLINE;
    let mut cursor: Option<String> = None;
    let mut seen_cursors: HashSet<String> = HashSet::new();
    let mut tools: Vec<Value> = Vec::new();
    let mut total_bytes = 0_usize;
    let mut pages = 0_usize;
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            return Err(SyncError::new("sync-deadline"));
        }
        let page = session
            .list_page(cursor.as_deref(), remaining)
            .await
            .map_err(SyncError::from)?;
        pages += 1;
        if pages > MCP_LIST_PAGES_MAX {
            return Err(SyncError::new("sync-pages-limit"));
        }
        let page_bytes = page.to_string().len();
        if page_bytes > MCP_LIST_PAGE_MAX_BYTES {
            return Err(SyncError::new("sync-page-limit"));
        }
        total_bytes = total_bytes.saturating_add(page_bytes);
        if total_bytes > MCP_LIST_TOTAL_MAX_BYTES {
            return Err(SyncError::new("sync-bytes-limit"));
        }
        let items = page
            .get("tools")
            .and_then(Value::as_array)
            .ok_or_else(|| SyncError::new("sync-tools-shape"))?;
        // An empty array with an advancing cursor is allowed.
        tools.extend(items.iter().cloned());
        if tools.len() > MCP_TOOLS_PER_SOURCE_MAX {
            return Err(SyncError::new("sync-tools-limit"));
        }
        let next = page
            .get("nextCursor")
            .and_then(Value::as_str)
            .filter(|cursor| !cursor.is_empty())
            .map(str::to_string);
        match next {
            None => break,
            Some(next) => {
                if !seen_cursors.insert(next.clone()) {
                    return Err(SyncError::new("sync-cursor-loop"));
                }
                cursor = Some(next);
            }
        }
    }
    Ok(tools)
}

fn prepare(
    source_id: &str,
    endpoint_hash: &str,
    tools: Vec<Value>,
) -> Result<Vec<NormalizedTool>, SyncError> {
    let mut normalized = Vec::with_capacity(tools.len());
    let mut seen = HashSet::new();
    for tool in tools {
        let item =
            descriptors::normalize_tool(source_id, endpoint_hash, &tool).map_err(SyncError::new)?;
        if !seen.insert(item.tool_id.clone()) {
            return Err(SyncError::new("sync-duplicate-name"));
        }
        normalized.push(item);
    }
    Ok(normalized)
}

fn publish(
    writer: &Arc<SqliteWriter>,
    source_id: &str,
    config_generation: i64,
    owner_principal: &str,
    endpoint_hash: &str,
    normalized: Vec<NormalizedTool>,
) -> Result<SyncOutcome, SyncError> {
    let now = now_ms();
    writer
        .write(|connection| {
            let transaction = connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(crate::database_error)?;
            let outcome = publish_inner(
                &transaction,
                source_id,
                config_generation,
                owner_principal,
                endpoint_hash,
                &normalized,
                now,
            )
            .map_err(|error| error.code.to_string())?;
            transaction.commit().map_err(crate::database_error)?;
            Ok(outcome)
        })
        .map_err(|code| SyncError {
            code: code_to_static(&code),
        })
}

fn publish_inner(
    connection: &rusqlite::Connection,
    source_id: &str,
    config_generation: i64,
    owner_principal: &str,
    endpoint_hash: &str,
    normalized: &[NormalizedTool],
    now: i64,
) -> Result<SyncOutcome, SyncError> {
    // A settings change during the fetch invalidates this snapshot before anything is written.
    let recorded =
        mcp_repository::source(connection, source_id).map_err(|_| SyncError::new("storage"))?;
    if let Some(recorded) = recorded.as_ref() {
        if recorded.config_generation != config_generation {
            return Err(SyncError::new("sync-generation-changed"));
        }
    }
    let other_tools: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM tool_selection_catalog WHERE source_id <> ?1",
            rusqlite::params![source_id],
            |row| row.get(0),
        )
        .map_err(|_| SyncError::new("storage"))?;
    if other_tools as usize + normalized.len() > MCP_TOOLS_PER_PROFILE_MAX {
        return Err(SyncError::new("sync-profile-limit"));
    }

    repository::upsert_source(connection, source_id, "mcp_http", owner_principal, true)
        .map_err(|_| SyncError::new("storage"))?;
    mcp_repository::upsert_source(connection, source_id, config_generation, endpoint_hash)
        .map_err(|_| SyncError::new("storage"))?;

    let mut existing: HashMap<String, ExistingTool> = HashMap::new();
    {
        let mut statement = connection
            .prepare(
                "SELECT id, enabled, current_revision_id FROM tool_selection_catalog
                  WHERE source_id = ?1",
            )
            .map_err(|_| SyncError::new("storage"))?;
        let rows = statement
            .query_map(rusqlite::params![source_id], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    ExistingTool {
                        enabled: row.get::<_, i64>(1)? == 1,
                        current_revision_id: row.get(2)?,
                    },
                ))
            })
            .map_err(|_| SyncError::new("storage"))?;
        for row in rows {
            let (tool_id, tool) = row.map_err(|_| SyncError::new("storage"))?;
            existing.insert(tool_id, tool);
        }
    }

    let mut outcome = SyncOutcome {
        published: normalized.len(),
        ..SyncOutcome::default()
    };
    let mut published_ids: HashSet<String> = HashSet::new();
    for tool in normalized {
        published_ids.insert(tool.tool_id.clone());
        let previous = existing.get(&tool.tool_id);
        let revision_exists = repository::revision_by_id(connection, &tool.revision_id)
            .map_err(|_| SyncError::new("storage"))?
            .is_some();
        let pointer_same = previous
            .map(|row| row.current_revision_id.as_deref() == Some(tool.revision_id.as_str()))
            .unwrap_or(false);
        let reappeared = previous.map(|row| !row.enabled).unwrap_or(false);
        if previous.is_none() {
            outcome.added += 1;
        } else if !pointer_same || reappeared {
            outcome.changed += 1;
            outcome.changed_tool_ids.push(tool.tool_id.clone());
        }
        if previous.is_some() && pointer_same && !reappeared {
            // Identical content already published: no write, no epoch movement.
            continue;
        }
        repository::upsert_tool(
            connection,
            &repository::NewTool {
                source_id,
                tool_id: &tool.tool_id,
                backend_key: &tool.entry.backend_key,
                enabled: true,
            },
        )
        .map_err(|_| SyncError::new("storage"))?;
        if !revision_exists {
            write_revision(connection, tool, now)?;
            outcome.new_revision_ids.push(tool.revision_id.clone());
        }
        repository::set_current_revision(connection, &tool.tool_id, &tool.revision_id)
            .map_err(|_| SyncError::new("storage"))?;
    }

    for (tool_id, row) in &existing {
        if published_ids.contains(tool_id) || !row.enabled {
            continue;
        }
        connection
            .execute(
                "UPDATE tool_selection_catalog SET enabled = 0 WHERE id = ?1",
                rusqlite::params![tool_id],
            )
            .map_err(|_| SyncError::new("storage"))?;
        outcome.disabled += 1;
        outcome.changed_tool_ids.push(tool_id.clone());
    }

    outcome.epoch_bumped = outcome.added > 0 || outcome.changed > 0 || outcome.disabled > 0;
    if outcome.epoch_bumped {
        repository::bump_epochs(connection, true, false, false)
            .map_err(|_| SyncError::new("storage"))?;
    }
    mcp_repository::mark_success(connection, source_id, now, config_generation)
        .map_err(|_| SyncError::new("storage"))?;
    Ok(outcome)
}

fn write_revision(
    connection: &rusqlite::Connection,
    tool: &NormalizedTool,
    now: i64,
) -> Result<(), SyncError> {
    let entry = &tool.entry;
    let search_text = entry.search_text();
    let schema_hash =
        descriptors::hex_sha256(descriptors::canonical_json_string(&entry.input_schema).as_bytes());
    let description_hash = tool.descriptor_hash.clone();
    repository::insert_revision(
        connection,
        &repository::NewRevision {
            revision_id: &tool.revision_id,
            tool_id: &tool.tool_id,
            schema_hash: &schema_hash,
            description_hash: &description_hash,
            input_schema: &entry.input_schema,
            output_schema: entry.output_schema.as_ref(),
            search_text: &search_text,
            operations: &entry.operations,
            objects: &entry.objects,
            effect: entry.effect,
            backend_binding: &entry.backend_binding,
            created_at: now,
        },
    )
    .map_err(|_| SyncError::new("storage"))?;
    for page in &entry.usage_pages {
        repository::upsert_usage_page_bounded(
            connection,
            &tool.revision_id,
            page.section,
            page.page,
            &page.text,
        )
        .map_err(|_| SyncError::new("storage"))?;
    }
    repository::upsert_fts(connection, &tool.revision_id, &search_text)
        .map_err(|_| SyncError::new("storage"))?;
    Ok(())
}

fn code_to_static(code: &str) -> &'static str {
    match code {
        "sync-generation-changed" => "sync-generation-changed",
        "sync-profile-limit" => "sync-profile-limit",
        "sync-deadline" => "sync-deadline",
        "sync-pages-limit" => "sync-pages-limit",
        "sync-page-limit" => "sync-page-limit",
        "sync-bytes-limit" => "sync-bytes-limit",
        "sync-tools-limit" => "sync-tools-limit",
        "sync-too-many" => "sync-tools-limit",
        "sync-cursor-loop" => "sync-cursor-loop",
        "sync-duplicate-name" => "sync-duplicate-name",
        "sync-tools-shape" => "sync-tools-shape",
        "tool-descriptor-name" => "tool-descriptor-name",
        "tool-descriptor-shape" => "tool-descriptor-shape",
        "tool-description-too-large" => "tool-description-too-large",
        "tool-schema-shape" => "tool-schema-shape",
        "tool-schema-too-large" => "tool-schema-too-large",
        "tool-schema-unsupported" => "tool-schema-unsupported",
        "tool-annotations-shape" => "tool-annotations-shape",
        "tool-meta-shape" => "tool-meta-shape",
        "tool-descriptor-too-large" => "tool-descriptor-too-large",
        "missing-token" => "missing-token",
        "protocol-version-mismatch" => "protocol-version-mismatch",
        "tools-capability-missing" => "tools-capability-missing",
        _ => "sync-error",
    }
}
