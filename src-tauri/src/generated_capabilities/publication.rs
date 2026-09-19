//! M2A publication: which verified capabilities become conversation tools, and the immutable
//! per-request offer snapshot.
//!
//! The configuration file is read once at startup (`SAAA_GENERATED_TOOLS_CONFIG`); changing it
//! requires a restart. It never contains candidate-provided data, and a broken file only disables
//! the generated tools, never SAAA itself.

use serde::Deserialize;
use serde_json::{json, Value};
use std::{
    collections::HashSet,
    env, fs,
    path::{Path, PathBuf},
};

use super::{
    contracts::{ResolvedCapability, WasmContract},
    errors::*,
};

pub const CONFIG_ENV: &str = "SAAA_GENERATED_TOOLS_CONFIG";
pub const TOOL_PREFIX: &str = "gc_";
pub const MAX_PUBLISHED_TOOLS: usize = 8;
pub const MAX_CONFIG_BYTES: usize = 64 * 1024;
/// The generated definition array (compact JSON) must stay at or below this size.
pub const MAX_DEFINITIONS_BYTES: usize = 32 * 1024;
/// A tool call's raw arguments must be at most this many bytes before any host spawn.
pub const MAX_INPUT_BYTES: usize = 16 * 1024;

/// Publication settings, loaded once from `SAAA_GENERATED_TOOLS_CONFIG`. A rejected file leaves
/// `enabled = false` and records a diagnostic that never echoes file contents.
#[derive(Clone, Debug, Default)]
pub struct GeneratedToolsConfig {
    pub enabled: bool,
    pub capability_ids: Vec<String>,
    pub diagnostic: Option<&'static str>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct GeneratedToolsConfigFile {
    format_version: u32,
    enabled: bool,
    capability_ids: Vec<String>,
}

impl GeneratedToolsConfig {
    pub fn disabled() -> Self {
        Self::default()
    }

    pub fn from_environment() -> Self {
        let path = env::var_os(CONFIG_ENV).map(PathBuf::from);
        Self::from_path(path.as_deref())
    }

    pub fn from_path(path: Option<&Path>) -> Self {
        let Some(path) = path else {
            return Self::disabled();
        };
        // A relative path would resolve against the process working directory, so
        // the published set could change with the launch directory. Only an
        // explicitly absolute path may enable publication (review P2).
        if !path.is_absolute() {
            return Self {
                enabled: false,
                capability_ids: Vec::new(),
                diagnostic: Some("generated tools config path must be absolute"),
            };
        }
        match Self::load(path) {
            Ok(config) => config,
            Err(diagnostic) => Self {
                enabled: false,
                capability_ids: Vec::new(),
                diagnostic: Some(diagnostic),
            },
        }
    }

    fn load(path: &Path) -> Result<Self, &'static str> {
        let metadata = fs::metadata(path).map_err(|_| "generated tools config is unreadable")?;
        if metadata.len() > MAX_CONFIG_BYTES as u64 {
            return Err("generated tools config is too large");
        }
        let bytes = fs::read(path).map_err(|_| "generated tools config is unreadable")?;
        if bytes.len() > MAX_CONFIG_BYTES {
            return Err("generated tools config is too large");
        }
        let file: GeneratedToolsConfigFile =
            serde_json::from_slice(&bytes).map_err(|_| "generated tools config is invalid")?;
        if file.format_version != 1 {
            return Err("unsupported generated tools config version");
        }
        if file.capability_ids.len() > MAX_PUBLISHED_TOOLS {
            return Err("too many published capabilities");
        }
        let mut seen = HashSet::new();
        for capability_id in &file.capability_ids {
            if capability_id.is_empty() || !seen.insert(capability_id.as_str()) {
                return Err("invalid or duplicate published capability id");
            }
        }
        Ok(Self {
            enabled: file.enabled,
            capability_ids: file.capability_ids,
            diagnostic: None,
        })
    }
}

/// One offered generated tool. The descriptor is provider-neutral; the OpenAI adapter lives in
/// [`super::tools`] and MCP can consume `name`/`input_schema` directly.
#[derive(Clone, Debug)]
pub struct GeneratedToolDescriptor {
    pub tool_name: String,
    pub description: String,
    pub input_schema: Value,
    pub resolved: ResolvedCapability,
}

/// The immutable offer for one provider request. It owns the mapping from the exact tool name it
/// offered to the revision that must run, so a later catalog change can never swap the target.
#[derive(Clone, Debug, Default)]
pub struct GeneratedToolSnapshot {
    descriptors: Vec<GeneratedToolDescriptor>,
    by_name: std::collections::HashMap<String, usize>,
}

impl GeneratedToolSnapshot {
    pub fn empty() -> Self {
        Self::default()
    }

    pub fn is_empty(&self) -> bool {
        self.descriptors.is_empty()
    }

    pub fn descriptors(&self) -> &[GeneratedToolDescriptor] {
        &self.descriptors
    }

    pub fn resolve(&self, tool_name: &str) -> Option<&ResolvedCapability> {
        self.by_name
            .get(tool_name)
            .map(|index| &self.descriptors[*index].resolved)
    }

    /// Provider-neutral definition array, sorted by tool name. Used for the 32 KiB size limit and
    /// as the source for provider adapters.
    pub fn definitions(&self) -> Vec<Value> {
        self.descriptors
            .iter()
            .map(|descriptor| {
                json!({
                    "name": descriptor.tool_name,
                    "description": descriptor.description,
                    "parameters": descriptor.input_schema,
                })
            })
            .collect()
    }

    /// Builds a snapshot from one catalog read. A malformed contract, a duplicated name or a
    /// definition array over the limit fails the whole build so the caller publishes nothing
    /// rather than silently truncating.
    pub fn build(resolved: Vec<ResolvedCapability>) -> CapabilityResult<Self> {
        let mut descriptors = Vec::with_capacity(resolved.len());
        let mut names = HashSet::new();
        for capability in resolved {
            let tool_name = tool_name(&capability.revision_id).ok_or_else(|| {
                CapabilityError::new(
                    CapabilityErrorCode::IntegrityError,
                    "capability revision id is not a UUID",
                )
            })?;
            if !names.insert(tool_name.clone()) {
                return error(
                    CapabilityErrorCode::Conflict,
                    "generated tool name is not unique in the offer",
                );
            }
            let input_schema = input_schema(&capability.contract)?;
            descriptors.push(GeneratedToolDescriptor {
                tool_name,
                description: description(&capability.contract),
                input_schema,
                resolved: capability,
            });
        }
        descriptors.sort_by(|left, right| left.tool_name.cmp(&right.tool_name));
        let by_name = descriptors
            .iter()
            .enumerate()
            .map(|(index, descriptor)| (descriptor.tool_name.clone(), index))
            .collect();
        let snapshot = Self {
            descriptors,
            by_name,
        };
        let bytes = serde_json::to_vec(&snapshot.definitions()).map_err(|_| {
            CapabilityError::new(
                CapabilityErrorCode::StorageError,
                "generated tool definitions are not serialisable",
            )
        })?;
        if bytes.len() > MAX_DEFINITIONS_BYTES {
            return error(
                CapabilityErrorCode::OutputLimit,
                "generated tool definitions exceed 32 KiB",
            );
        }
        Ok(snapshot)
    }
}

/// `gc_` + the revision UUID without hyphens. An invalid UUID is rejected, never re-derived from
/// a hash.
pub(crate) fn tool_name(revision_id: &str) -> Option<String> {
    uuid::Uuid::parse_str(revision_id)
        .ok()
        .map(|uuid| format!("{TOOL_PREFIX}{}", uuid.simple()))
}

fn input_schema(contract: &WasmContract) -> CapabilityResult<Value> {
    if contract.validate_subset().is_err() {
        return error(
            CapabilityErrorCode::UnsupportedContract,
            "capability contract is outside the boolean subset",
        );
    }
    let properties = contract
        .fields
        .iter()
        .map(|field| (field.name.clone(), json!({ "type": "boolean" })))
        .collect::<serde_json::Map<_, _>>();
    let required = contract
        .fields
        .iter()
        .map(|field| Value::String(field.name.clone()))
        .collect::<Vec<_>>();
    Ok(json!({
        "type": "object",
        "properties": properties,
        "required": required,
        "additionalProperties": false,
    }))
}

/// Fixed host-managed description. Candidate free text is never inserted into the prompt.
fn description(contract: &WasmContract) -> String {
    let fields = contract
        .fields
        .iter()
        .map(|field| field.name.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "Verified boolean predicate. Returns a boolean result when all listed boolean inputs are supplied. \
         Input fields: {fields}."
    )
}
