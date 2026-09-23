//! Host-managed external MCP source configuration.
//!
//! The LLM can never supply a URL, token, source id or grant. Everything here is parsed from a
//! separate JSON file referenced by the `mcpSourcesPath` field of the existing tool-selection
//! configuration. A malformed document rejects the whole file; the manager then keeps the last
//! valid configuration instead of partially applying it.

use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::path::Path;

use super::{MCP_SOURCES_FILE_MAX_BYTES, MCP_SOURCES_MAX};

/// Diagnostic codes shown to operators. They never contain a token or URL fragment.
pub type McpConfigDiagnostic = &'static str;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum McpGrantScope {
    User,
    Project,
}

impl McpGrantScope {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Project => "project",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct McpGrantSpec {
    pub tool_name: String,
    pub scope_kind: McpGrantScope,
    /// Required when `scope_kind` is project; the manager verifies the id exists on the host.
    pub project_id: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct McpSourceSpec {
    pub id: String,
    pub url: String,
    pub enabled: bool,
    /// Name of the environment variable holding the bearer token. The token is resolved at
    /// connect time and never stored in the database or this struct.
    pub bearer_token_env: Option<String>,
    pub grants: Vec<McpGrantSpec>,
}

impl McpSourceSpec {
    /// `None` when no token is configured or the configured variable is unset/empty. An enabled
    /// source with a missing token is not connectable; the manager records it as unavailable.
    pub fn resolve_token(&self) -> Option<String> {
        let name = self.bearer_token_env.as_deref()?;
        let value = std::env::var(name).ok()?;
        if value.is_empty() {
            None
        } else {
            Some(value)
        }
    }

    pub fn token_configured(&self) -> bool {
        self.bearer_token_env.is_some()
    }

    /// SHA-256 over the normalized endpoint with secrets removed. The URL has no userinfo (that
    /// is rejected), the query and fragment are dropped, and default ports are normalized away.
    pub fn endpoint_hash(&self) -> String {
        endpoint_hash(&self.url)
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct McpSources {
    pub sources: Vec<McpSourceSpec>,
}

impl McpSources {
    pub fn get(&self, id: &str) -> Option<&McpSourceSpec> {
        self.sources.iter().find(|source| source.id == id)
    }

    /// Parses and validates a whole document. Any syntax, type, duplicate-id, unknown-field or
    /// URL problem rejects the file; there is no partial accept.
    pub fn parse(bytes: &[u8]) -> Result<Self, McpConfigDiagnostic> {
        if bytes.len() as u64 > MCP_SOURCES_FILE_MAX_BYTES {
            return Err("mcp-sources-too-large");
        }
        let document: SourcesDocument =
            serde_json::from_slice(bytes).map_err(|_| "mcp-sources-invalid-json")?;
        document.validate()
    }

    pub fn load(path: &Path) -> Result<Self, McpConfigDiagnostic> {
        let metadata = std::fs::metadata(path).map_err(|_| "mcp-sources-unreadable")?;
        if metadata.len() > MCP_SOURCES_FILE_MAX_BYTES {
            return Err("mcp-sources-too-large");
        }
        let bytes = std::fs::read(path).map_err(|_| "mcp-sources-unreadable")?;
        Self::parse(&bytes)
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SourcesDocument {
    pub(super) format_version: u32,
    pub(super) sources: Vec<SourceDocument>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SourceDocument {
    pub(super) id: String,
    pub(super) url: String,
    #[serde(default = "default_enabled")]
    pub(super) enabled: bool,
    pub(super) bearer_token_env: Option<String>,
    #[serde(default)]
    pub(super) grants: Vec<GrantDocument>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct GrantDocument {
    pub(super) tool_name: String,
    pub(super) scope_kind: String,
    pub(super) project_id: Option<String>,
}

fn default_enabled() -> bool {
    true
}

impl SourcesDocument {
    fn validate(self) -> Result<McpSources, McpConfigDiagnostic> {
        if self.format_version != 1 {
            return Err("mcp-sources-version-unknown");
        }
        if self.sources.len() > MCP_SOURCES_MAX {
            return Err("mcp-sources-too-many");
        }
        let mut seen = std::collections::HashSet::new();
        let mut sources = Vec::with_capacity(self.sources.len());
        for source in self.sources {
            validate_source_id(&source.id)?;
            if !seen.insert(source.id.clone()) {
                return Err("mcp-sources-duplicate-id");
            }
            validate_url(&source.url)?;
            if let Some(name) = source.bearer_token_env.as_deref() {
                if name.is_empty() || name.len() > 128 {
                    return Err("mcp-sources-token-env-invalid");
                }
            }
            let mut grants = Vec::with_capacity(source.grants.len());
            for grant in source.grants {
                grants.push(validate_grant(grant)?);
            }
            sources.push(McpSourceSpec {
                id: source.id,
                url: source.url,
                enabled: source.enabled,
                bearer_token_env: source.bearer_token_env,
                grants,
            });
        }
        Ok(McpSources { sources })
    }
}

fn validate_source_id(id: &str) -> Result<(), McpConfigDiagnostic> {
    let mut characters = id.chars();
    let Some(first) = characters.next() else {
        return Err("mcp-sources-id-invalid");
    };
    if !first.is_ascii_lowercase() && !first.is_ascii_digit() {
        return Err("mcp-sources-id-invalid");
    }
    if id.len() > 64 {
        return Err("mcp-sources-id-invalid");
    }
    if !id.chars().all(|character| {
        character.is_ascii_lowercase()
            || character.is_ascii_digit()
            || character == '_'
            || character == '-'
    }) {
        return Err("mcp-sources-id-invalid");
    }
    Ok(())
}

fn validate_url(raw: &str) -> Result<(), McpConfigDiagnostic> {
    let url = url::Url::parse(raw).map_err(|_| "mcp-sources-url-invalid")?;
    if !url.username().is_empty() || url.password().is_some() {
        return Err("mcp-sources-url-invalid");
    }
    if url.fragment().is_some() {
        return Err("mcp-sources-url-invalid");
    }
    match url.scheme() {
        "https" => Ok(()),
        "http" => match url.host() {
            Some(url::Host::Ipv4(address)) if address.is_loopback() => Ok(()),
            Some(url::Host::Ipv6(address)) if address.is_loopback() => Ok(()),
            _ => Err("mcp-sources-url-insecure"),
        },
        _ => Err("mcp-sources-url-scheme"),
    }
}

fn validate_grant(grant: GrantDocument) -> Result<McpGrantSpec, McpConfigDiagnostic> {
    if grant.tool_name.is_empty() || grant.tool_name.len() > 256 {
        return Err("mcp-sources-grant-invalid");
    }
    let (scope_kind, project_id) = match grant.scope_kind.as_str() {
        "user" => {
            if grant.project_id.is_some() {
                return Err("mcp-sources-grant-invalid");
            }
            (McpGrantScope::User, None)
        }
        "project" => {
            let project_id = grant
                .project_id
                .filter(|value| !value.is_empty() && value.len() <= 160)
                .ok_or("mcp-sources-grant-invalid")?;
            (McpGrantScope::Project, Some(project_id))
        }
        _ => return Err("mcp-sources-grant-invalid"),
    };
    Ok(McpGrantSpec {
        tool_name: grant.tool_name,
        scope_kind,
        project_id,
    })
}

/// Normalized endpoint hash. The query string is excluded so a token placed in a query parameter
/// can never be embedded in a revision binding.
pub fn endpoint_hash(raw: &str) -> String {
    let normalized = url::Url::parse(raw)
        .map(|mut url| {
            url.set_query(None);
            url.set_fragment(None);
            url.to_string()
        })
        .unwrap_or_else(|_| raw.to_string());
    let digest = Sha256::digest(normalized.as_bytes());
    let mut output = String::with_capacity(64);
    for byte in digest {
        output.push_str(&format!("{byte:02x}"));
    }
    output
}
