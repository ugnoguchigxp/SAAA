//! Host configuration and credential loading for the SAAA MCP server.
//!
//! The configuration is read once at startup from the absolute path in
//! `SAAA_TOOL_GATEWAY_MCP_CONFIG`; there is no hot reload. The bearer token lives in a separate
//! owner-only file and is never copied into the configuration struct, a log line, the database or
//! a response.

use serde::Deserialize;
use std::path::{Path, PathBuf};

/// Environment variable holding the absolute path of the MCP server configuration document.
pub const MCP_SERVER_CONFIG_ENV: &str = "SAAA_TOOL_GATEWAY_MCP_CONFIG";
pub const MCP_SERVER_CONFIG_FORMAT_VERSION: u32 = 1;
pub const MCP_SERVER_CONFIG_MAX_BYTES: u64 = 64 * 1024;
pub const MCP_SERVER_TOKEN_MAX_BYTES: u64 = 4 * 1024;
/// Minimum entropy of the credential: at least 32 random bytes encoded as base64url.
pub const MCP_SERVER_TOKEN_MIN_BYTES: usize = 32;

/// Fixed bind address. The server is loopback-only; no interface fallback exists.
pub const MCP_SERVER_BIND_ADDRESS: &str = "127.0.0.1";
/// Single endpoint path.
pub const MCP_SERVER_ENDPOINT: &str = "/mcp";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct McpServerConfig {
    pub enabled: bool,
    /// `0` binds an ephemeral port and is reserved for the in-process acceptance tests.
    pub port: u16,
    pub token_file: Option<PathBuf>,
    pub project_id: Option<String>,
}

/// Loading `None` means "no listener": either the environment variable is unset or the document
/// disables the server. `Err` is a safe diagnostic code; it never includes a path or a secret.
pub fn from_environment() -> Result<Option<McpServerConfig>, &'static str> {
    let Some(path) = std::env::var_os(MCP_SERVER_CONFIG_ENV).map(PathBuf::from) else {
        return Ok(None);
    };
    let config = load(&path)?;
    from_config(config)
}

/// Applies the runtime rules that depend on the document rather than the environment. Kept
/// separate from `from_environment` so the port and enabled rules are testable without mutating
/// process-global environment variables.
fn from_config(config: McpServerConfig) -> Result<Option<McpServerConfig>, &'static str> {
    if !config.enabled {
        return Ok(None);
    }
    // Port 0 is reserved for the in-process tests via `start`; a production document must name a
    // real port so an operator cannot accidentally bind an ephemeral one.
    if config.port == 0 {
        return Err("mcp-server-config-port-invalid");
    }
    Ok(Some(config))
}

pub fn load(path: &Path) -> Result<McpServerConfig, &'static str> {
    if !path.is_absolute() {
        return Err("mcp-server-config-path-not-absolute");
    }
    let metadata = std::fs::metadata(path).map_err(|_| "mcp-server-config-unreadable")?;
    if metadata.len() > MCP_SERVER_CONFIG_MAX_BYTES {
        return Err("mcp-server-config-too-large");
    }
    let bytes = std::fs::read(path).map_err(|_| "mcp-server-config-unreadable")?;
    parse(&bytes)
}

pub fn parse(bytes: &[u8]) -> Result<McpServerConfig, &'static str> {
    let document: ConfigDocument =
        serde_json::from_slice(bytes).map_err(|_| "mcp-server-config-invalid-json")?;
    document.validate()
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ConfigDocument {
    format_version: u32,
    enabled: bool,
    port: u16,
    token_file: Option<String>,
    #[serde(default)]
    project_id: Option<String>,
}

impl ConfigDocument {
    fn validate(self) -> Result<McpServerConfig, &'static str> {
        if self.format_version != MCP_SERVER_CONFIG_FORMAT_VERSION {
            return Err("mcp-server-config-version-unknown");
        }
        // `u16` already enforces 0..=65535; 0 is only meaningful for the in-process tests.
        let token_file = match self.token_file {
            None if self.enabled => return Err("mcp-server-config-token-missing"),
            None => None,
            Some(path) if path.is_empty() => return Err("mcp-server-config-token-missing"),
            Some(path) => {
                let path = PathBuf::from(path);
                if !path.is_absolute() {
                    return Err("mcp-server-config-token-path-not-absolute");
                }
                Some(path)
            }
        };
        let project_id = match self.project_id {
            None => None,
            Some(value) if value.is_empty() || value.len() > 160 => {
                return Err("mcp-server-config-project-invalid")
            }
            Some(value) => Some(value),
        };
        Ok(McpServerConfig {
            enabled: self.enabled,
            port: self.port,
            token_file,
            project_id,
        })
    }
}

/// Reads, permission-checks and decodes the bearer token. The value is returned only to the
/// server that must compare it; it is never stored anywhere else.
pub fn load_token(config: &McpServerConfig) -> Result<String, &'static str> {
    let path = config
        .token_file
        .as_deref()
        .ok_or("mcp-server-token-missing")?;
    let metadata = std::fs::metadata(path).map_err(|_| "mcp-server-token-unreadable")?;
    if !metadata.is_file() {
        return Err("mcp-server-token-not-a-file");
    }
    if metadata.len() > MCP_SERVER_TOKEN_MAX_BYTES {
        return Err("mcp-server-token-too-large");
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o077 != 0 {
            return Err("mcp-server-token-permissions");
        }
    }
    let raw = std::fs::read(path).map_err(|_| "mcp-server-token-unreadable")?;
    let text = std::str::from_utf8(&raw).map_err(|_| "mcp-server-token-invalid")?;
    // Exactly one trailing newline is tolerated; any other whitespace or control character is
    // rejected so a truncated or pasted value cannot silently compare equal.
    let text = text.strip_suffix('\n').unwrap_or(text);
    if text.is_empty()
        || text
            .chars()
            .any(|character| character.is_whitespace() || character.is_control())
    {
        return Err("mcp-server-token-invalid");
    }
    use base64::Engine;
    let decoded = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(text)
        .or_else(|_| base64::engine::general_purpose::URL_SAFE.decode(text))
        .map_err(|_| "mcp-server-token-invalid")?;
    if decoded.len() < MCP_SERVER_TOKEN_MIN_BYTES {
        return Err("mcp-server-token-too-short");
    }
    Ok(text.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write_token(directory: &Path, contents: &[u8], mode: u32) -> PathBuf {
        let path = directory.join("token");
        let mut file = std::fs::File::create(&path).expect("create");
        file.write_all(contents).expect("write");
        drop(file);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode)).expect("mode");
        }
        #[cfg(not(unix))]
        let _ = mode;
        path
    }

    fn token_value() -> String {
        use base64::Engine;
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode([7u8; 32])
    }

    #[test]
    fn unset_environment_means_no_listener() {
        // The helper is pure; parse of a disabled document returns enabled=false.
        let document = br#"{"formatVersion":1,"enabled":false,"port":43127,"tokenFile":"/tmp/x","projectId":null}"#;
        let config = parse(document).expect("parse");
        assert!(!config.enabled);
    }

    #[test]
    fn runtime_rules_reject_ephemeral_ports_and_disable_cleanly() {
        let enabled = McpServerConfig {
            enabled: true,
            port: 43127,
            token_file: Some(PathBuf::from("/tmp/token")),
            project_id: None,
        };
        assert_eq!(from_config(enabled.clone()).unwrap(), Some(enabled.clone()));
        let ephemeral = McpServerConfig { port: 0, ..enabled };
        assert_eq!(
            from_config(ephemeral).unwrap_err(),
            "mcp-server-config-port-invalid"
        );
        let disabled = McpServerConfig {
            enabled: false,
            port: 0,
            token_file: None,
            project_id: None,
        };
        assert!(from_config(disabled).unwrap().is_none());
    }

    #[test]
    fn invalid_documents_are_rejected() {
        let cases: &[(&[u8], &str)] = &[
            (b"not json", "mcp-server-config-invalid-json"),
            (
                br#"{"formatVersion":9,"enabled":true,"port":1,"tokenFile":"/tmp/x"}"#,
                "mcp-server-config-version-unknown",
            ),
            (
                br#"{"formatVersion":1,"enabled":true,"port":70000,"tokenFile":"/tmp/x"}"#,
                "mcp-server-config-invalid-json",
            ),
            (
                br#"{"formatVersion":1,"enabled":true,"port":1}"#,
                "mcp-server-config-token-missing",
            ),
            (
                br#"{"formatVersion":1,"enabled":true,"port":1,"tokenFile":"relative"}"#,
                "mcp-server-config-token-path-not-absolute",
            ),
            (
                br#"{"formatVersion":1,"enabled":true,"port":1,"tokenFile":"/tmp/x","unexpected":1}"#,
                "mcp-server-config-invalid-json",
            ),
        ];
        for (bytes, expected) in cases {
            let error = parse(bytes).unwrap_err();
            assert_eq!(error, *expected, "for {}", String::from_utf8_lossy(bytes));
        }
    }

    #[test]
    fn token_requires_owner_only_permissions_and_entropy() {
        let directory = tempfile::tempdir().expect("tempdir");
        let path = write_token(directory.path(), token_value().as_bytes(), 0o600);
        let config = McpServerConfig {
            enabled: true,
            port: 1,
            token_file: Some(path.clone()),
            project_id: None,
        };
        assert_eq!(load_token(&config).unwrap(), token_value());

        // A trailing newline is tolerated exactly once.
        let _ = write_token(
            directory.path(),
            format!("{}\n", token_value()).as_bytes(),
            0o600,
        );
        assert_eq!(load_token(&config).unwrap(), token_value());

        #[cfg(unix)]
        {
            let path = write_token(directory.path(), token_value().as_bytes(), 0o644);
            let config = McpServerConfig {
                token_file: Some(path),
                ..config.clone()
            };
            assert_eq!(
                load_token(&config).unwrap_err(),
                "mcp-server-token-permissions"
            );
        }

        let path = write_token(directory.path(), b"short\n", 0o600);
        let config = McpServerConfig {
            token_file: Some(path),
            ..config.clone()
        };
        assert!(load_token(&config).is_err());

        // Exactly one trailing newline is tolerated; any other whitespace or control character is
        // rejected so a pasted or truncated value cannot silently compare equal.
        for contents in [
            format!("{}\n\n", token_value()),
            format!("{} {}", &token_value()[..8], &token_value()[8..]),
            format!("{}\t", token_value()),
        ] {
            let path = write_token(directory.path(), contents.as_bytes(), 0o600);
            let config = McpServerConfig {
                token_file: Some(path),
                ..config.clone()
            };
            assert_eq!(load_token(&config).unwrap_err(), "mcp-server-token-invalid");
        }
    }
}
