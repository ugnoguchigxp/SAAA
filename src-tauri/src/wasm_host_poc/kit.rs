use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs,
    path::{Component, Path, PathBuf},
};

use super::contracts::{is_hash, HOST_PROTOCOL};

#[derive(Clone, Debug)]
pub(super) struct TrustedKit {
    pub(super) directory: PathBuf,
    pub(super) kit_json_hash: String,
    pub(super) package_hash: String,
    pub(super) provenance_commit: String,
    pub(super) bun_version: String,
}

#[derive(Debug)]
pub(super) struct ValidatedKit {
    pub(super) root: PathBuf,
    pub(super) provenance_commit: String,
    pub(super) bun_version: String,
}

impl ValidatedKit {
    pub(super) fn runtime_path(&self) -> PathBuf {
        self.root.join("runtime/capability-host-cli.ts")
    }

    pub(super) fn candidate_path(&self) -> PathBuf {
        self.root.join("candidate/capability.json")
    }

    pub(super) fn request_schema(&self) -> Result<serde_json::Value, String> {
        read_json(&self.root.join("request.schema.json"))
    }

    pub(super) fn response_schema(&self) -> Result<serde_json::Value, String> {
        read_json(&self.root.join("response.schema.json"))
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct KitInfo {
    protocol: String,
    bun_version: String,
    package_hash: String,
    fixture: bool,
    provenance: Provenance,
    acceptance: String,
    files: BTreeMap<String, String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Provenance {
    commit: Option<String>,
    dirty: Option<bool>,
}

pub(super) fn validate(trusted: &TrustedKit) -> Result<ValidatedKit, String> {
    if !trusted.directory.is_absolute() {
        return Err("kit directory must be absolute".into());
    }
    let root = trusted
        .directory
        .canonicalize()
        .map_err(|error| format!("kit directory is unavailable: {error}"))?;
    let kit_path = root.join("kit.json");
    let kit_bytes = read_regular(&kit_path)?;
    if digest(&kit_bytes) != trusted.kit_json_hash {
        return Err("untrusted kit.json hash".into());
    }
    let info: KitInfo =
        serde_json::from_slice(&kit_bytes).map_err(|error| format!("invalid kit.json: {error}"))?;
    if info.protocol != HOST_PROTOCOL
        || info.package_hash != trusted.package_hash
        || !is_hash(&info.package_hash)
        || info.bun_version != trusted.bun_version
        || info.provenance.commit.as_deref() != Some(&trusted.provenance_commit)
        || info.provenance.dirty != Some(false)
        || !info.fixture
        || info.acceptance != "not-run"
    {
        return Err("kit provenance does not match the trusted record".into());
    }
    for required in [
        "candidate/capability.json",
        "runtime/capability-host-cli.ts",
        "runtime/capability-worker.ts",
        "runtime/capability-invoke-worker.ts",
        "request.schema.json",
        "response.schema.json",
        "vectors.json",
        "verification.json",
    ] {
        if !info.files.contains_key(required) {
            return Err(format!("kit is missing required file {required}"));
        }
    }
    for (relative, expected_hash) in &info.files {
        if !safe_relative_path(relative) || !is_hash(expected_hash) {
            return Err(format!("invalid kit file record {relative}"));
        }
        let path = root.join(relative);
        let bytes = read_regular(&path)?;
        if digest(&bytes) != *expected_hash {
            return Err(format!("kit file hash mismatch: {relative}"));
        }
    }
    Ok(ValidatedKit {
        root,
        provenance_commit: info.provenance.commit.unwrap_or_default(),
        bun_version: info.bun_version,
    })
}

fn read_json(path: &Path) -> Result<serde_json::Value, String> {
    let bytes = read_regular(path)?;
    serde_json::from_slice(&bytes)
        .map_err(|error| format!("invalid JSON {}: {error}", path.display()))
}

fn read_regular(path: &Path) -> Result<Vec<u8>, String> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| format!("cannot inspect {}: {error}", path.display()))?;
    if !metadata.file_type().is_file() || metadata.file_type().is_symlink() {
        return Err(format!(
            "expected regular non-symlink file: {}",
            path.display()
        ));
    }
    fs::read(path).map_err(|error| format!("cannot read {}: {error}", path.display()))
}

fn safe_relative_path(value: &str) -> bool {
    let path = Path::new(value);
    !value.is_empty()
        && !path.is_absolute()
        && path
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
}

pub(super) fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
