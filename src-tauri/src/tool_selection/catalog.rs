//! Catalog registration, usage-page import and explicit grants. This is the only write path that
//! creates tools and revisions; no LLM-facing tool can call it and an import is never a grant.

use rusqlite::Connection;
use serde_json::Value;
use sha2::{Digest, Sha256};

use super::contracts::*;
use super::repository::{self, NewRevision, NewTool};

/// Fixed per-field caps for the search document. The total is additionally capped at 4 KiB.
const FIELD_CAP: usize = 700;
const SEARCH_DOC_MAX: usize = SEARCH_TEXT_MAX_BYTES;

#[derive(Clone, Debug)]
pub struct UsagePage {
    pub section: &'static str,
    pub page: i64,
    pub text: String,
}

#[derive(Clone, Debug)]
pub struct CatalogEntry {
    pub tool_id: String,
    pub backend_key: String,
    pub title: String,
    pub purpose: String,
    pub operations: Vec<String>,
    pub objects: Vec<String>,
    pub suitable: Vec<String>,
    pub unsuitable: Vec<String>,
    pub required_inputs: Vec<String>,
    pub input_schema: Value,
    pub output_schema: Option<Value>,
    pub effect: &'static str,
    pub usage_pages: Vec<UsagePage>,
    pub backend_binding: Value,
}

impl CatalogEntry {
    /// Fixed order: title / purpose / operations / objects / suitable / unsuitable / required
    /// inputs. Field cap and a 4 KiB whole-document cap are applied with UTF-8 boundaries.
    pub fn search_text(&self) -> String {
        let mut sections = Vec::new();
        push_section(&mut sections, "title", std::slice::from_ref(&self.title));
        push_section(
            &mut sections,
            "purpose",
            std::slice::from_ref(&self.purpose),
        );
        push_section(&mut sections, "operations", &self.operations);
        push_section(&mut sections, "objects", &self.objects);
        push_section(&mut sections, "suitable", &self.suitable);
        push_section(&mut sections, "unsuitable", &self.unsuitable);
        push_section(&mut sections, "required inputs", &self.required_inputs);
        repository::truncate_utf8(&sections.join("\n"), SEARCH_DOC_MAX).to_string()
    }
}

fn push_section(sections: &mut Vec<String>, label: &str, values: &[String]) {
    if values.is_empty() {
        return;
    }
    let joined = values.join("; ");
    let bounded = repository::truncate_utf8(&joined, FIELD_CAP);
    sections.push(format!("{label}: {bounded}"));
}

pub fn hex_sha256(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut output = String::with_capacity(64);
    for byte in digest {
        output.push_str(&format!("{byte:02x}"));
    }
    output
}

pub fn canonical_json(value: &Value) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| "null".to_string())
}

/// Registers one revision and points the tool at it in a single transaction, advancing the
/// catalog epoch exactly once. The revision must belong to the same tool as the pointer.
pub fn register_revision(
    connection: &Connection,
    principal_id: &str,
    source_id: &str,
    entry: &CatalogEntry,
    revision_id: &str,
    created_at: i64,
) -> ToolSelectionResult<()> {
    let source_kind = "llang";
    repository::upsert_source(connection, source_id, source_kind, principal_id, true)
        .map_err(|_| ToolSelectionError::storage())?;
    repository::upsert_tool(
        connection,
        &NewTool {
            source_id,
            tool_id: &entry.tool_id,
            backend_key: &entry.backend_key,
            enabled: true,
        },
    )
    .map_err(|_| ToolSelectionError::storage())?;

    if repository::revision_by_id(connection, revision_id)
        .map_err(|_| ToolSelectionError::storage())?
        .is_some()
    {
        // Re-importing the same immutable revision is a no-op, not a new epoch.
        return Ok(());
    }

    let search_text = entry.search_text();
    let schema_hash = hex_sha256(canonical_json(&entry.input_schema).as_bytes());
    let description_hash = hex_sha256(search_text.as_bytes());
    repository::insert_revision(
        connection,
        &NewRevision {
            revision_id,
            tool_id: &entry.tool_id,
            schema_hash: &schema_hash,
            description_hash: &description_hash,
            input_schema: &entry.input_schema,
            output_schema: entry.output_schema.as_ref(),
            search_text: &search_text,
            operations: &entry.operations,
            objects: &entry.objects,
            effect: entry.effect,
            backend_binding: &entry.backend_binding,
            created_at,
        },
    )
    .map_err(|_| ToolSelectionError::storage())?;

    for page in &entry.usage_pages {
        repository::upsert_usage_page_bounded(
            connection,
            revision_id,
            page.section,
            page.page,
            &page.text,
        )
        .map_err(|_| ToolSelectionError::storage())?;
    }
    repository::upsert_fts(connection, revision_id, &search_text)
        .map_err(|_| ToolSelectionError::storage())?;
    repository::set_current_revision(connection, &entry.tool_id, revision_id)
        .map_err(|_| ToolSelectionError::storage())?;
    repository::bump_epochs(connection, true, false, false)
        .map_err(|_| ToolSelectionError::storage())?;
    Ok(())
}

/// One-time migration of the previously configured L-Lang IDs into explicit user-scope grants.
/// Registering the catalog itself is never a grant.
pub fn seed_user_grants(
    connection: &Connection,
    principal_id: &str,
    tool_ids: &[String],
) -> ToolSelectionResult<usize> {
    let mut granted = 0;
    for tool_id in tool_ids {
        let exists = repository::tool_by_id(connection, tool_id)
            .map_err(|_| ToolSelectionError::storage())?
            .is_some();
        if !exists {
            continue;
        }
        let already = repository::grant_exists(connection, principal_id, tool_id, None)
            .map_err(|_| ToolSelectionError::storage())?;
        repository::upsert_grant(connection, principal_id, tool_id, "user", principal_id)
            .map_err(|_| ToolSelectionError::storage())?;
        if !already {
            granted += 1;
        }
    }
    if granted > 0 {
        repository::bump_epochs(connection, false, true, false)
            .map_err(|_| ToolSelectionError::storage())?;
    }
    Ok(granted)
}

pub fn grant_user(
    connection: &Connection,
    principal_id: &str,
    tool_id: &str,
) -> ToolSelectionResult<()> {
    let exists = repository::tool_by_id(connection, tool_id)
        .map_err(|_| ToolSelectionError::storage())?
        .is_some();
    if !exists {
        return Err(ToolSelectionError::invalid());
    }
    repository::upsert_grant(connection, principal_id, tool_id, "user", principal_id)
        .map_err(|_| ToolSelectionError::storage())?;
    repository::bump_epochs(connection, false, true, false)
        .map_err(|_| ToolSelectionError::storage())?;
    Ok(())
}

pub fn grant_project(
    connection: &Connection,
    principal_id: &str,
    tool_id: &str,
    project_id: &str,
) -> ToolSelectionResult<()> {
    let exists = repository::tool_by_id(connection, tool_id)
        .map_err(|_| ToolSelectionError::storage())?
        .is_some();
    if !exists {
        return Err(ToolSelectionError::invalid());
    }
    repository::upsert_grant(connection, principal_id, tool_id, "project", project_id)
        .map_err(|_| ToolSelectionError::storage())?;
    repository::bump_epochs(connection, false, true, false)
        .map_err(|_| ToolSelectionError::storage())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::persistence::schema::initialize_database;
    use crate::tool_selection::repository;

    fn entry(tool_id: &str) -> CatalogEntry {
        CatalogEntry {
            tool_id: tool_id.to_string(),
            backend_key: tool_id.to_string(),
            title: tool_id.to_string(),
            purpose: "Catalog grant test tool.".to_string(),
            operations: vec!["search".to_string()],
            objects: vec!["decision_record".to_string()],
            suitable: vec!["tests".to_string()],
            unsuitable: vec!["production".to_string()],
            required_inputs: vec!["query".to_string()],
            input_schema: serde_json::json!({
                "type": "object",
                "properties": { "query": { "type": "string" } },
                "required": ["query"],
                "additionalProperties": false
            }),
            output_schema: None,
            effect: "read",
            usage_pages: vec![],
            backend_binding: serde_json::json!({}),
        }
    }

    #[test]
    fn registration_is_not_a_grant_and_grants_are_scoped() {
        let connection = rusqlite::Connection::open_in_memory().expect("in-memory");
        initialize_database(&connection).expect("schema");
        register_revision(&connection, "P1", "llang", &entry("t"), "t-rev1", 1).expect("register");

        // Registering the catalog is never a grant.
        assert!(!repository::grant_exists(&connection, "P1", "t", None).expect("grant check"));

        grant_user(&connection, "P1", "t").expect("grant user");
        assert!(repository::grant_exists(&connection, "P1", "t", None).expect("grant check"));

        grant_project(&connection, "P2", "t", "A").expect("grant project");
        assert!(repository::grant_exists(&connection, "P2", "t", Some("A")).expect("grant check"));
        assert!(!repository::grant_exists(&connection, "P2", "t", Some("B")).expect("grant check"));
    }
}
