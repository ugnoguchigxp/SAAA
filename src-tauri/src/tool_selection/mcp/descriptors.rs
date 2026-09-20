//! Stable identity and descriptor normalization for external MCP tools.
//!
//! The descriptor hash is computed over the *full* tool definition (description, input/output
//! schema, annotations and explicit management metadata), never over the 4 KiB search document.
//! JSON object keys are sorted recursively while array order is preserved, so reordering keys does
//! not create a new revision.

use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

use super::super::catalog::{CatalogEntry, UsagePage};
use super::super::contracts::DESCRIBE_RESPONSE_MAX_BYTES;
use super::{MCP_DESCRIPTION_MAX_BYTES, MCP_PROTOCOL_VERSION, MCP_SCHEMA_MAX_BYTES};

/// A tool definition normalized to the exact bytes that participate in `revision_id`.
#[derive(Clone, Debug)]
pub struct NormalizedTool {
    pub tool_id: String,
    pub revision_id: String,
    pub tool_name: String,
    pub endpoint_hash: String,
    pub descriptor_hash: String,
    pub entry: CatalogEntry,
}

/// Stable `tool_id`: same source + same tool name always maps to the same id, and the same name
/// in a different source maps to a different id.
pub fn tool_id(source_id: &str, tool_name: &str) -> String {
    let canonical = canonical_json_string(&Value::Array(vec![
        Value::String(source_id.to_string()),
        Value::String(tool_name.to_string()),
    ]));
    format!("mcpt_{}", hex_sha256(canonical.as_bytes()))
}

/// Stable `revision_id` for the observed descriptor. Changing the endpoint, schema, description,
/// annotations or management metadata produces a new revision; re-syncing identical content does
/// not.
pub fn revision_id(tool_id: &str, endpoint_hash: &str, descriptor: &Value) -> String {
    let canonical = canonical_json_string(&Value::Array(vec![
        Value::String(tool_id.to_string()),
        Value::String(endpoint_hash.to_string()),
        descriptor.clone(),
    ]));
    format!("mcpr_{}", hex_sha256(canonical.as_bytes()))
}

pub fn hex_sha256(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut output = String::with_capacity(64);
    for byte in digest {
        output.push_str(&format!("{byte:02x}"));
    }
    output
}

/// Recursively sorts object keys and serializes compactly. Array order is preserved.
pub fn canonical_json_string(value: &Value) -> String {
    let mut output = String::new();
    write_canonical(&mut output, value);
    output
}

fn write_canonical(output: &mut String, value: &Value) {
    match value {
        Value::Object(map) => {
            output.push('{');
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            for (index, key) in keys.iter().enumerate() {
                if index > 0 {
                    output.push(',');
                }
                output.push_str(&serde_json::to_string(key).unwrap_or_else(|_| "\"\"".into()));
                output.push(':');
                write_canonical(output, &map[*key]);
            }
            output.push('}');
        }
        Value::Array(items) => {
            output.push('[');
            for (index, item) in items.iter().enumerate() {
                if index > 0 {
                    output.push(',');
                }
                write_canonical(output, item);
            }
            output.push(']');
        }
        other => {
            output.push_str(&serde_json::to_string(other).unwrap_or_else(|_| "null".into()));
        }
    }
}

/// Normalizes one `tools/list` entry. Returns a fixed diagnostic code on any contract violation;
/// the whole sync then fails instead of substituting `{}` for an unsupported schema.
pub fn normalize_tool(
    source_id: &str,
    endpoint_hash: &str,
    value: &Value,
) -> Result<NormalizedTool, &'static str> {
    let object = value.as_object().ok_or("tool-descriptor-shape")?;
    let tool_name = object
        .get("name")
        .and_then(Value::as_str)
        .filter(|name| !name.is_empty() && name.len() <= 256)
        .ok_or("tool-descriptor-name")?
        .to_string();
    let title = object
        .get("title")
        .and_then(Value::as_str)
        .unwrap_or(&tool_name)
        .to_string();
    let description = object
        .get("description")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    if description.len() > MCP_DESCRIPTION_MAX_BYTES {
        return Err("tool-description-too-large");
    }

    let input_schema = match object.get("inputSchema") {
        None | Some(Value::Null) => Value::Object(Map::new()),
        Some(schema) => {
            validate_schema(schema)?;
            schema.clone()
        }
    };
    let output_schema = match object.get("outputSchema") {
        None | Some(Value::Null) => None,
        Some(schema) => {
            validate_schema(schema)?;
            Some(schema.clone())
        }
    };

    let annotations = object
        .get("annotations")
        .filter(|value| !value.is_null())
        .cloned();
    if let Some(annotations) = annotations.as_ref() {
        if !annotations.is_object() {
            return Err("tool-annotations-shape");
        }
    }

    let meta = object
        .get("_meta")
        .filter(|value| !value.is_null())
        .cloned();
    if let Some(meta) = meta.as_ref() {
        if !meta.is_object() {
            return Err("tool-meta-shape");
        }
    }
    let management = meta
        .as_ref()
        .and_then(|meta| meta.get("saaa"))
        .filter(|value| value.is_object())
        .cloned();

    let effect = match management
        .as_ref()
        .and_then(|value| value.get("effect"))
        .and_then(Value::as_str)
    {
        Some("pure") => "pure",
        Some("read") => "read",
        Some("write") => "write",
        _ => "unknown",
    };
    let operations = management_strings(management.as_ref(), "operations");
    let objects = management_strings(management.as_ref(), "objects");
    let suitable = management_strings(management.as_ref(), "suitable");
    let unsuitable = management_strings(management.as_ref(), "unsuitable");
    let required_inputs = required_inputs(&input_schema);

    let tool_id = tool_id(source_id, &tool_name);
    let descriptor = normalized_descriptor(
        &tool_name,
        &title,
        &description,
        &input_schema,
        output_schema.as_ref(),
        annotations.as_ref(),
        management.as_ref(),
    );
    let descriptor_hash = hex_sha256(canonical_json_string(&descriptor).as_bytes());
    let revision_id = revision_id(&tool_id, endpoint_hash, &descriptor);

    let entry = CatalogEntry {
        tool_id: tool_id.clone(),
        backend_key: tool_name.clone(),
        title: title.clone(),
        purpose: description.clone(),
        operations,
        objects,
        suitable,
        unsuitable,
        required_inputs,
        input_schema: input_schema.clone(),
        output_schema: output_schema.clone(),
        effect,
        usage_pages: usage_pages(&description),
        backend_binding: serde_json::json!({
            "kind": "mcp_http",
            "sourceId": source_id,
            "toolName": tool_name,
            "endpointHash": endpoint_hash,
        }),
    };
    check_describe_envelope(&entry, &revision_id)?;

    Ok(NormalizedTool {
        tool_id,
        revision_id,
        tool_name,
        endpoint_hash: endpoint_hash.to_string(),
        descriptor_hash,
        entry,
    })
}

fn validate_schema(schema: &Value) -> Result<(), &'static str> {
    if !schema.is_object() {
        return Err("tool-schema-shape");
    }
    let encoded = canonical_json_string(schema);
    if encoded.len() > MCP_SCHEMA_MAX_BYTES {
        return Err("tool-schema-too-large");
    }
    jsonschema::validator_for(schema).map_err(|_| "tool-schema-unsupported")?;
    Ok(())
}

fn management_strings(management: Option<&Value>, key: &str) -> Vec<String> {
    management
        .and_then(|value| value.get(key))
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

fn required_inputs(schema: &Value) -> Vec<String> {
    schema
        .get("required")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

fn normalized_descriptor(
    tool_name: &str,
    title: &str,
    description: &str,
    input_schema: &Value,
    output_schema: Option<&Value>,
    annotations: Option<&Value>,
    management: Option<&Value>,
) -> Value {
    serde_json::json!({
        "name": tool_name,
        "title": title,
        "description": description,
        "inputSchema": input_schema,
        "outputSchema": output_schema,
        "annotations": annotations,
        "management": management,
    })
}

/// Splits the full description into contiguous 8 KiB UTF-8 pages so `tools_describe` can return
/// it without ever exceeding the existing usage-page bound.
pub fn usage_pages(description: &str) -> Vec<UsagePage> {
    if description.is_empty() {
        return Vec::new();
    }
    let mut pages = Vec::new();
    let mut remaining = description;
    let mut index = 0_i64;
    while !remaining.is_empty() {
        let end = page_boundary(remaining, super::super::contracts::USAGE_PAGE_MAX_BYTES);
        let (chunk, rest) = remaining.split_at(end);
        pages.push(UsagePage {
            section: "usage",
            page: index,
            text: chunk.to_string(),
        });
        remaining = rest;
        index += 1;
    }
    pages
}

fn page_boundary(text: &str, max: usize) -> usize {
    if text.len() <= max {
        return text.len();
    }
    let mut end = max;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    if end == 0 {
        // A single scalar value can never exceed four bytes, so this is unreachable for max >= 4.
        text.chars().next().map(char::len_utf8).unwrap_or(0)
    } else {
        end
    }
}

/// The `contract` describe body must fit the existing 16 KiB envelope. A descriptor that cannot
/// be described can never be invoked, so the sync fails instead of publishing a dead tool.
fn check_describe_envelope(entry: &CatalogEntry, revision_id: &str) -> Result<(), &'static str> {
    let body = serde_json::json!({
        "revisionId": revision_id,
        "title": entry.title,
        "inputSchema": entry.input_schema,
        "outputSchema": entry.output_schema,
    });
    let bytes = serde_json::to_vec(&body).map_err(|_| "tool-descriptor-shape")?;
    if bytes.len() > DESCRIBE_RESPONSE_MAX_BYTES {
        return Err("tool-descriptor-too-large");
    }
    Ok(())
}

/// Protocol version recorded in session bindings.
pub const PROTOCOL_VERSION: &str = MCP_PROTOCOL_VERSION;
