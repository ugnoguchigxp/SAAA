use serde::Deserialize;
use serde_json::Value;
use std::{
    collections::BTreeMap,
    fs,
    path::{Component, Path, PathBuf},
};

use super::super::{
    contracts::{is_hash, sha256_hex},
    errors::*,
};

/// Trusted runtime configuration, supplied by the SAAA process launcher through
/// `SAAA_LLANG_RUNTIME_CONFIG`. Candidates never contribute environment variables here.
#[derive(Clone, Debug)]
pub struct RuntimeConfig {
    pub format_version: u32,
    pub enabled: bool,
    pub bun_path: PathBuf,
    pub runtime_root: PathBuf,
    pub expected_runtime_digest: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RuntimeConfigFile {
    pub(super) format_version: u32,
    pub(super) enabled: bool,
    pub(super) bun_path: String,
    pub(super) runtime_root: String,
    pub(super) expected_runtime_digest: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RuntimeManifest {
    pub(super) format_version: u32,
    pub(super) entrypoint: String,
    pub(super) request_schema: String,
    pub(super) response_schema: String,
    pub(super) bun_version: String,
    pub(super) llang_version: Option<String>,
    #[serde(rename = "llangDirty")]
    pub(super) _llang_dirty: Option<bool>,
    pub(super) files: BTreeMap<String, String>,
}

#[derive(Debug)]
pub struct TrustedRuntime {
    pub root: PathBuf,
    pub entrypoint: PathBuf,
    pub entrypoint_name: String,
    pub request_schema: Value,
    pub response_schema: Value,
    pub digest: String,
    pub bun_version: String,
    pub llang_version: String,
    pub(super) files: BTreeMap<String, String>,
}

impl TrustedRuntime {
    /// Re-establishes, immediately before a spawn, that the bundle on disk is still the trusted
    /// one. The verified paths are exactly the paths the command executes.
    pub fn revalidate(&self) -> CapabilityResult<()> {
        let manifest = read_manifest(&self.root)?;
        if manifest.format_version != 1
            || manifest.entrypoint != self.entrypoint_name
            || manifest.files != self.files
        {
            return error(
                CapabilityErrorCode::Unavailable,
                "runtime bundle changed since it was trusted",
            );
        }
        for (name, expected) in &self.files {
            if sha256_hex(&read_regular(&self.root.join(name))?) != *expected {
                return error(CapabilityErrorCode::Unavailable, "runtime file changed");
            }
        }
        Ok(())
    }
}

fn read_manifest(root: &Path) -> CapabilityResult<RuntimeManifest> {
    serde_json::from_slice(&read_regular(&root.join("manifest.json"))?).map_err(|_| {
        CapabilityError::new(
            CapabilityErrorCode::Unavailable,
            "runtime manifest is invalid",
        )
    })
}

pub fn load_config(path: &Path) -> CapabilityResult<RuntimeConfig> {
    let unavailable =
        |message: &'static str| CapabilityError::new(CapabilityErrorCode::Unavailable, message);
    let bytes = fs::read(path).map_err(|_| unavailable("runtime config is unreadable"))?;
    let file: RuntimeConfigFile =
        serde_json::from_slice(&bytes).map_err(|_| unavailable("runtime config is invalid"))?;
    if file.format_version != 1 {
        return Err(unavailable("unsupported runtime config version"));
    }
    Ok(RuntimeConfig {
        format_version: file.format_version,
        enabled: file.enabled,
        bun_path: PathBuf::from(file.bun_path),
        runtime_root: PathBuf::from(file.runtime_root),
        expected_runtime_digest: file.expected_runtime_digest,
    })
}

/// Validates the trusted runtime bundle. The digest is computed from the runtime's own
/// manifest and compared against the administrator-set value; a freshly computed digest is
/// never adopted as the trust anchor.
pub fn validate(config: &RuntimeConfig) -> CapabilityResult<TrustedRuntime> {
    if !is_hash(&config.expected_runtime_digest) {
        return error(
            CapabilityErrorCode::Unavailable,
            "expected runtime digest is not configured",
        );
    }
    if !config.bun_path.is_absolute() || !config.bun_path.is_file() {
        return error(
            CapabilityErrorCode::Unavailable,
            "configured Bun path is unavailable",
        );
    }
    if !config.runtime_root.is_absolute() {
        return error(
            CapabilityErrorCode::Unavailable,
            "runtime root must be absolute",
        );
    }
    let root = config.runtime_root.canonicalize().map_err(|_| {
        CapabilityError::new(
            CapabilityErrorCode::Unavailable,
            "runtime root is unavailable",
        )
    })?;
    let manifest = read_manifest(&root)?;
    if manifest.format_version != 1 || manifest.files.is_empty() {
        return error(
            CapabilityErrorCode::Unavailable,
            "unsupported runtime manifest",
        );
    }
    for (name, hash) in &manifest.files {
        if !safe_relative(name) || !is_hash(hash) {
            return error(
                CapabilityErrorCode::Unavailable,
                "invalid runtime file record",
            );
        }
    }
    for required in [
        &manifest.entrypoint,
        &manifest.request_schema,
        &manifest.response_schema,
    ] {
        if !manifest.files.contains_key(required) {
            return error(
                CapabilityErrorCode::Unavailable,
                "runtime manifest is missing a required file",
            );
        }
    }
    let digest = runtime_digest(&manifest.files);
    if digest != config.expected_runtime_digest {
        return error(
            CapabilityErrorCode::Unavailable,
            "runtime digest does not match the configured trust value",
        );
    }
    for (name, expected) in &manifest.files {
        let bytes = read_regular(&root.join(name))?;
        if sha256_hex(&bytes) != *expected {
            return error(
                CapabilityErrorCode::Unavailable,
                "runtime file hash mismatch",
            );
        }
    }
    let request_schema =
        serde_json::from_slice(&read_regular(&root.join(&manifest.request_schema))?).map_err(
            |_| CapabilityError::new(CapabilityErrorCode::Unavailable, "invalid request schema"),
        )?;
    let response_schema =
        serde_json::from_slice(&read_regular(&root.join(&manifest.response_schema))?).map_err(
            |_| CapabilityError::new(CapabilityErrorCode::Unavailable, "invalid response schema"),
        )?;
    Ok(TrustedRuntime {
        entrypoint: root.join(&manifest.entrypoint),
        entrypoint_name: manifest.entrypoint,
        root,
        request_schema,
        response_schema,
        digest,
        bun_version: manifest.bun_version,
        llang_version: manifest.llang_version.unwrap_or_default(),
        files: manifest.files,
    })
}

/// Digest over the runtime's fixed file list: sorted `name\0hash\n` records.
pub fn runtime_digest(files: &BTreeMap<String, String>) -> String {
    let mut text = String::new();
    for (name, hash) in files {
        text.push_str(name);
        text.push('\u{0}');
        text.push_str(hash);
        text.push('\n');
    }
    sha256_hex(text.as_bytes())
}

fn safe_relative(value: &str) -> bool {
    let path = Path::new(value);
    !value.is_empty()
        && !path.is_absolute()
        && path
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
}

fn read_regular(path: &Path) -> CapabilityResult<Vec<u8>> {
    let metadata = fs::symlink_metadata(path).map_err(|_| {
        CapabilityError::new(
            CapabilityErrorCode::Unavailable,
            "runtime file is unavailable",
        )
    })?;
    if !metadata.file_type().is_file() || metadata.file_type().is_symlink() {
        return error(
            CapabilityErrorCode::Unavailable,
            "runtime files must be regular non-symlink files",
        );
    }
    fs::read(path).map_err(|_| {
        CapabilityError::new(
            CapabilityErrorCode::Unavailable,
            "runtime file is unreadable",
        )
    })
}
