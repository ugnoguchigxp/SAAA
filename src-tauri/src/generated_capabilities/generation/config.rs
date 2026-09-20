//! Generation configuration and the host-registered request catalog (plan 12.1, 12.2).
//!
//! `SAAA_LLANG_GENERATION_CONFIG` points at an absolute JSON file. Unknown fields are rejected
//! and `enabled` may not be omitted. The request file is read separately and its bytes are hashed
//! at load time so a later edit to a registered request/suite/metadata is detected.

use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::{Path, PathBuf},
};

use super::super::contracts::sha256_hex;
use super::super::errors::CapabilityError;
use super::super::errors::{CapabilityErrorCode, CapabilityResult};
use super::contracts::{encode_error, GenerationErrorCode, GENERATION_FORMAT_VERSION};

pub const GENERATION_CONFIG_ENV: &str = "SAAA_LLANG_GENERATION_CONFIG";
pub const KIT_FORMAT_VERSION: u32 = 1;
pub const REQUESTS_FORMAT_VERSION: u32 = 1;
pub const MAX_CONFIG_BYTES: u64 = 64 * 1024;
pub const MAX_REQUESTS_BYTES: u64 = 1024 * 1024;
pub const MAX_REQUEST_ENTRIES: usize = 32;
pub const MAX_REQUEST_FIELDS: usize = 8;
pub const MIN_REQUEST_FIELDS: usize = 1;
pub const MAX_PURPOSE_BYTES: usize = 4096;

/// Administrator configuration for the generation kit.
#[derive(Clone, Debug)]
pub struct GenerationConfig {
    pub format_version: u32,
    pub enabled: bool,
    pub bun_path: PathBuf,
    pub kit_root: PathBuf,
    pub expected_kit_digest: String,
    pub requests_path: PathBuf,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct GenerationConfigFile {
    format_version: u32,
    enabled: bool,
    bun_path: String,
    kit_root: String,
    expected_kit_digest: String,
    requests_path: String,
}

/// One host-registered generation request. `request_hash`/`suite_hash`/`metadata_hash` are fixed
/// at load time; the job workspace copies these bytes so a running generation never re-reads a
/// file that changed underneath it.
#[derive(Clone, Debug)]
pub struct RegisteredRequest {
    pub id: String,
    pub capability_id: String,
    pub purpose: String,
    pub fields: Vec<String>,
    pub request_path: PathBuf,
    pub request_hash: String,
    pub suite_path: PathBuf,
    pub suite_hash: String,
    pub metadata_path: PathBuf,
    pub metadata_hash: String,
    pub acceptance_id: String,
    pub scope: RequestScope,
    pub allow_create: bool,
    pub allow_update: bool,
    pub auto_activate: bool,
    pub grant_on_create: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum RequestScope {
    User,
    Project { id: String },
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RequestsFile {
    format_version: u32,
    entries: Vec<RequestEntryFile>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RequestEntryFile {
    id: String,
    capability_id: String,
    purpose: String,
    fields: Vec<String>,
    request_path: String,
    suite_path: String,
    metadata_path: String,
    acceptance_id: String,
    scope: ScopeFile,
    allow_create: bool,
    allow_update: bool,
    auto_activate: bool,
    grant_on_create: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ScopeFile {
    kind: String,
    #[serde(default)]
    id: Option<String>,
}

fn read_bounded(
    path: &Path,
    limit: u64,
    code: GenerationErrorCode,
    message: &'static str,
) -> CapabilityResult<Vec<u8>> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|_| encode_error(code, "configuration file is unavailable"))?;
    if !metadata.file_type().is_file() || metadata.file_type().is_symlink() {
        return Err(encode_error(code, "configuration must be a regular file"));
    }
    if metadata.len() > limit {
        return Err(encode_error(code, message));
    }
    fs::read(path).map_err(|_| encode_error(code, "configuration file is unreadable"))
}

pub fn load_config(path: &Path) -> CapabilityResult<GenerationConfig> {
    let bytes = read_bounded(
        path,
        MAX_CONFIG_BYTES,
        GenerationErrorCode::Unavailable,
        "generation config exceeds 64 KiB",
    )?;
    let file: GenerationConfigFile = serde_json::from_slice(&bytes).map_err(|_| {
        encode_error(
            GenerationErrorCode::Unavailable,
            "generation config is invalid",
        )
    })?;
    if file.format_version != GENERATION_FORMAT_VERSION {
        return Err(encode_error(
            GenerationErrorCode::Unavailable,
            "unsupported generation config version",
        ));
    }
    let bun_path = PathBuf::from(&file.bun_path);
    let kit_root = PathBuf::from(&file.kit_root);
    let requests_path = PathBuf::from(&file.requests_path);
    if !bun_path.is_absolute()
        || !kit_root.is_absolute()
        || !requests_path.is_absolute()
        || !bun_path.is_file()
    {
        return Err(encode_error(
            GenerationErrorCode::Unavailable,
            "generation config paths must be absolute and the Bun path must exist",
        ));
    }
    if file.expected_kit_digest.len() != 64
        || !file
            .expected_kit_digest
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(encode_error(
            GenerationErrorCode::Unavailable,
            "expected kit digest is not a 64-hex value",
        ));
    }
    Ok(GenerationConfig {
        format_version: file.format_version,
        enabled: file.enabled,
        bun_path,
        kit_root,
        expected_kit_digest: file.expected_kit_digest,
        requests_path,
    })
}

/// Reads the registered request catalog, validating every entry and hashing its three files.
pub fn load_requests(path: &Path) -> CapabilityResult<Vec<RegisteredRequest>> {
    let bytes = read_bounded(
        path,
        MAX_REQUESTS_BYTES,
        GenerationErrorCode::InvalidInput,
        "requests file exceeds 1 MiB",
    )?;
    let file: RequestsFile = serde_json::from_slice(&bytes).map_err(|_| {
        encode_error(
            GenerationErrorCode::InvalidInput,
            "requests file is invalid",
        )
    })?;
    if file.format_version != REQUESTS_FORMAT_VERSION {
        return Err(encode_error(
            GenerationErrorCode::InvalidInput,
            "unsupported requests format version",
        ));
    }
    if file.entries.len() > MAX_REQUEST_ENTRIES {
        return Err(encode_error(
            GenerationErrorCode::InvalidInput,
            "too many registered requests",
        ));
    }
    let mut entries = Vec::with_capacity(file.entries.len());
    for entry in file.entries {
        entries.push(validate_entry(entry)?);
    }
    let mut ids = entries
        .iter()
        .map(|entry| entry.id.as_str())
        .collect::<Vec<_>>();
    ids.sort_unstable();
    let unique = ids.iter().collect::<std::collections::HashSet<_>>();
    if unique.len() != ids.len() {
        return Err(encode_error(
            GenerationErrorCode::InvalidInput,
            "registered request ids must be unique",
        ));
    }
    Ok(entries)
}

fn validate_entry(entry: RequestEntryFile) -> CapabilityResult<RegisteredRequest> {
    if !is_request_id(&entry.id) {
        return Err(encode_error(
            GenerationErrorCode::InvalidInput,
            "registered request id is malformed",
        ));
    }
    if !is_capability_id(&entry.capability_id) {
        return Err(encode_error(
            GenerationErrorCode::InvalidInput,
            "registered capability id is malformed",
        ));
    }
    if entry.purpose.is_empty() || entry.purpose.len() > MAX_PURPOSE_BYTES {
        return Err(encode_error(
            GenerationErrorCode::InvalidInput,
            "registered purpose is out of range",
        ));
    }
    if entry.fields.len() < MIN_REQUEST_FIELDS || entry.fields.len() > MAX_REQUEST_FIELDS {
        return Err(encode_error(
            GenerationErrorCode::InvalidInput,
            "registered input field count is out of range",
        ));
    }
    let mut seen = std::collections::HashSet::new();
    for field in &entry.fields {
        if !is_field_identifier(field) || !seen.insert(field.as_str()) {
            return Err(encode_error(
                GenerationErrorCode::InvalidInput,
                "registered input fields must be unique ASCII identifiers",
            ));
        }
    }
    if !is_request_id(&entry.acceptance_id) {
        return Err(encode_error(
            GenerationErrorCode::InvalidInput,
            "registered acceptance id is malformed",
        ));
    }
    let scope = match entry.scope.kind.as_str() {
        "user" => {
            if entry.scope.id.is_some() {
                return Err(encode_error(
                    GenerationErrorCode::InvalidInput,
                    "user scope must not carry an id",
                ));
            }
            RequestScope::User
        }
        "project" => {
            let id = entry.scope.id.ok_or_else(|| {
                encode_error(
                    GenerationErrorCode::InvalidInput,
                    "project scope requires an id",
                )
            })?;
            if id.is_empty() || id.len() > 160 {
                return Err(encode_error(
                    GenerationErrorCode::InvalidInput,
                    "project scope id is out of range",
                ));
            }
            RequestScope::Project { id }
        }
        _ => {
            return Err(encode_error(
                GenerationErrorCode::InvalidInput,
                "unknown registered scope kind",
            ))
        }
    };
    let request_path = absolute_existing(&entry.request_path)?;
    let suite_path = absolute_existing(&entry.suite_path)?;
    let metadata_path = absolute_existing(&entry.metadata_path)?;
    let request_bytes = read_registered_file(&request_path)?;
    validate_request_contract(&request_bytes, &entry.capability_id, &entry.fields)?;
    let request_hash = sha256_hex(&request_bytes);
    let suite_hash = hash_file(&suite_path)?;
    let metadata_hash = hash_file(&metadata_path)?;
    Ok(RegisteredRequest {
        id: entry.id,
        capability_id: entry.capability_id,
        purpose: entry.purpose,
        fields: entry.fields,
        request_path,
        request_hash,
        suite_path,
        suite_hash,
        metadata_path,
        metadata_hash,
        acceptance_id: entry.acceptance_id,
        scope,
        allow_create: entry.allow_create,
        allow_update: entry.allow_update,
        auto_activate: entry.auto_activate,
        grant_on_create: entry.grant_on_create,
    })
}

fn absolute_existing(value: &str) -> CapabilityResult<PathBuf> {
    let path = PathBuf::from(value);
    if !path.is_absolute() || !path.is_file() {
        return Err(encode_error(
            GenerationErrorCode::InvalidInput,
            "registered file must be an existing absolute path",
        ));
    }
    Ok(path)
}

fn hash_file(path: &Path) -> CapabilityResult<String> {
    Ok(sha256_hex(&read_registered_file(path)?))
}

/// The request's id and contract field order/names must match the registered entry, so the model
/// prompt and the package build use the same contract the host approved.
fn validate_request_contract(
    request_bytes: &[u8],
    capability_id: &str,
    fields: &[String],
) -> CapabilityResult<()> {
    let value: serde_json::Value = serde_json::from_slice(request_bytes).map_err(|_| {
        encode_error(
            GenerationErrorCode::InvalidInput,
            "the registered request is not valid JSON",
        )
    })?;
    if value.get("id").and_then(serde_json::Value::as_str) != Some(capability_id) {
        return Err(encode_error(
            GenerationErrorCode::InvalidInput,
            "the registered request id does not match the capability id",
        ));
    }
    let names = value
        .get("contract")
        .and_then(|contract| contract.get("fields"))
        .and_then(serde_json::Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|field| field.get("name").and_then(serde_json::Value::as_str))
                .collect::<Vec<_>>()
        })
        .ok_or_else(|| {
            encode_error(
                GenerationErrorCode::InvalidInput,
                "the registered request has no contract fields",
            )
        })?;
    if names.len() != fields.len() || names.iter().zip(fields).any(|(left, right)| *left != right) {
        return Err(encode_error(
            GenerationErrorCode::InvalidInput,
            "the registered request contract does not match the registered fields",
        ));
    }
    Ok(())
}

fn read_registered_file(path: &Path) -> CapabilityResult<Vec<u8>> {
    let metadata = fs::symlink_metadata(path).map_err(|_| {
        CapabilityError::new(
            CapabilityErrorCode::InvalidInput,
            "registered file is unreadable",
        )
    })?;
    if !metadata.file_type().is_file()
        || metadata.len() > super::super::limits::MAX_CANDIDATE_FILE_BYTES
    {
        return Err(encode_error(
            GenerationErrorCode::InvalidInput,
            "registered file is not a regular file within the 1 MiB package limit",
        ));
    }
    fs::read(path).map_err(|_| {
        CapabilityError::new(
            CapabilityErrorCode::StorageError,
            "registered file is unreadable",
        )
    })
}

fn is_request_id(value: &str) -> bool {
    if value.is_empty() || value.len() > 64 {
        return false;
    }
    let mut bytes = value.bytes();
    match bytes.next() {
        Some(first) if first.is_ascii_lowercase() || first.is_ascii_digit() => {}
        _ => return false,
    }
    bytes.all(|byte| {
        byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_' || byte == b'-'
    })
}

fn is_capability_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
}

fn is_field_identifier(value: &str) -> bool {
    if value.is_empty() || value.len() > 64 {
        return false;
    }
    let mut bytes = value.bytes();
    match bytes.next() {
        Some(first) if first.is_ascii_alphabetic() || first == b'_' => {}
        _ => return false,
    }
    bytes.all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "saaa-generation-config-{tag}-{}",
            uuid::Uuid::new_v4().simple()
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write(path: &Path, bytes: &[u8]) {
        let mut file = fs::File::create(path).unwrap();
        file.write_all(bytes).unwrap();
    }

    fn sample_requests(dir: &Path) -> PathBuf {
        let request = dir.join("request.json");
        let suite = dir.join("tests.json");
        let metadata = dir.join("metadata.json");
        write(
            &request,
            serde_json::json!({
                "version": 2,
                "id": "req-cap",
                "body": "x",
                "contract": {
                    "version": 1,
                    "fields": [
                        { "name": "enabled", "kind": "boolean", "values": [], "nullable": false, "undefinable": false, "optional": false },
                        { "name": "suspended", "kind": "boolean", "values": [], "nullable": false, "undefinable": false, "optional": false }
                    ]
                }
            })
            .to_string()
            .as_bytes(),
        );
        write(&suite, b"{\"version\":1}");
        write(&metadata, b"{\"id\":\"req-cap\"}");
        let path = dir.join("requests.json");
        write(
            &path,
            serde_json::json!({
                "formatVersion": 1,
                "entries": [{
                    "id": "req-1",
                    "capabilityId": "req-cap",
                    "purpose": "Decide whether a user may proceed.",
                    "fields": ["enabled", "suspended"],
                    "requestPath": request.to_string_lossy(),
                    "suitePath": suite.to_string_lossy(),
                    "metadataPath": metadata.to_string_lossy(),
                    "acceptanceId": "acc-1",
                    "scope": { "kind": "user" },
                    "allowCreate": true,
                    "allowUpdate": false,
                    "autoActivate": true,
                    "grantOnCreate": true
                }]
            })
            .to_string()
            .as_bytes(),
        );
        path
    }

    fn sample_config(dir: &Path) -> PathBuf {
        let bun = dir.join("bun");
        write(&bun, b"#!/bin/sh\n");
        let requests = sample_requests(dir);
        let path = dir.join("generation.json");
        write(
            &path,
            serde_json::json!({
                "formatVersion": 1,
                "enabled": true,
                "bunPath": bun.to_string_lossy(),
                "kitRoot": dir.to_string_lossy(),
                "expectedKitDigest": "a".repeat(64),
                "requestsPath": requests.to_string_lossy()
            })
            .to_string()
            .as_bytes(),
        );
        path
    }

    #[test]
    fn config_round_trips_and_requires_enabled() {
        let dir = temp_dir("config");
        let path = sample_config(&dir);
        let config = load_config(&path).unwrap();
        assert!(config.enabled);
        assert_eq!(config.format_version, 1);

        let mut value: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        value.as_object_mut().unwrap().remove("enabled");
        write(&path, value.to_string().as_bytes());
        assert!(load_config(&path).is_err());

        value
            .as_object_mut()
            .unwrap()
            .insert("enabled".into(), true.into());
        value
            .as_object_mut()
            .unwrap()
            .insert("unknown".into(), true.into());
        write(&path, value.to_string().as_bytes());
        assert!(load_config(&path).is_err());
    }

    #[test]
    fn requests_are_hashed_at_load_and_validated() {
        let dir = temp_dir("requests");
        let path = sample_requests(&dir);
        let entries = load_requests(&path).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].fields, vec!["enabled", "suspended"]);
        assert_eq!(entries[0].scope, RequestScope::User);
        assert_eq!(entries[0].request_hash.len(), 64);

        let mut value: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        value["entries"][0]["fields"] = serde_json::json!(["enabled", "enabled"]);
        write(&path, value.to_string().as_bytes());
        assert!(load_requests(&path).is_err());

        value["entries"][0]["fields"] = serde_json::json!(["enabled", "9bad"]);
        write(&path, value.to_string().as_bytes());
        assert!(load_requests(&path).is_err());
    }

    #[test]
    fn request_contract_mismatch_is_rejected() {
        let dir = temp_dir("contract");
        let path = sample_requests(&dir);
        let mut value: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        value["entries"][0]["fields"] = serde_json::json!(["suspended", "enabled"]);
        write(&path, value.to_string().as_bytes());
        assert!(load_requests(&path).is_err());
    }

    #[test]
    fn duplicate_request_ids_are_rejected() {
        let dir = temp_dir("dupes");
        let path = sample_requests(&dir);
        let mut value: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        let entry = value["entries"][0].clone();
        value["entries"].as_array_mut().unwrap().push(entry);
        write(&path, value.to_string().as_bytes());
        assert!(load_requests(&path).is_err());
    }
}
