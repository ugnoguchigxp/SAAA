use std::ffi::OsString;
use std::fs::{File, OpenOptions};
use std::io::Read;
use std::path::Path;
use zeroize::Zeroizing;

use super::API_TOKEN_ENV;

const CREDENTIAL_FILE: &str = ".ssh/larm";
const MAX_TOKEN_BYTES: usize = 4_096;
const MAX_FILE_BYTES: u64 = ("LARM_API_TOKEN=".len() + MAX_TOKEN_BYTES + 1) as u64;

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub(crate) enum CredentialSource {
    ProcessEnv,
    UserFile,
    #[allow(dead_code)]
    Missing,
}

impl CredentialSource {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::ProcessEnv => "process-env",
            Self::UserFile => "user-file",
            Self::Missing => "missing",
        }
    }
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub(crate) struct CredentialObservation {
    pub(crate) credential_configured: bool,
    pub(crate) credential_source: CredentialSource,
}

pub(crate) struct LoadedCredential {
    token: Zeroizing<String>,
    observation: CredentialObservation,
}

impl std::fmt::Debug for LoadedCredential {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("LoadedCredential")
            .field(
                "credentialConfigured",
                &self.observation.credential_configured,
            )
            .field(
                "credentialSource",
                &self.observation.credential_source.as_str(),
            )
            .finish()
    }
}

impl LoadedCredential {
    pub(crate) fn token(&self) -> &str {
        self.token.as_str()
    }

    #[cfg(test)]
    pub(crate) fn observation(&self) -> CredentialObservation {
        self.observation
    }
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub(crate) struct CredentialLoadError {
    code: &'static str,
}

impl CredentialLoadError {
    fn new(code: &'static str) -> Self {
        Self { code }
    }

    pub(crate) fn code(self) -> &'static str {
        self.code
    }

    #[cfg(test)]
    pub(crate) fn observation(self) -> CredentialObservation {
        CredentialObservation {
            credential_configured: false,
            credential_source: CredentialSource::Missing,
        }
    }
}

#[cfg_attr(test, allow(dead_code))]
pub(crate) fn load() -> Result<LoadedCredential, CredentialLoadError> {
    let home =
        std::env::var_os("HOME").ok_or_else(|| CredentialLoadError::new("credential_missing"))?;
    let environment = std::env::var_os(API_TOKEN_ENV);
    load_from(environment, Path::new(&home), current_uid())
}

#[cfg(test)]
pub(crate) fn load_environment_only_for_test() -> Result<LoadedCredential, CredentialLoadError> {
    let token = std::env::var_os(API_TOKEN_ENV)
        .map(parse_environment)
        .transpose()?
        .ok_or_else(|| CredentialLoadError::new("credential_missing"))?;
    Ok(LoadedCredential {
        token,
        observation: CredentialObservation {
            credential_configured: true,
            credential_source: CredentialSource::ProcessEnv,
        },
    })
}

fn load_from(
    environment: Option<OsString>,
    home: &Path,
    expected_uid: u32,
) -> Result<LoadedCredential, CredentialLoadError> {
    let environment = environment.map(parse_environment).transpose()?;
    let path = home.join(CREDENTIAL_FILE);
    let file = read_file(&path, expected_uid)?;
    let (token, source) = match (environment, file) {
        (Some(environment), Some(file)) if environment.as_str() != file.as_str() => {
            return Err(CredentialLoadError::new("credential_conflict"));
        }
        (Some(environment), _) => (environment, CredentialSource::ProcessEnv),
        (None, Some(file)) => (file, CredentialSource::UserFile),
        (None, None) => return Err(CredentialLoadError::new("credential_missing")),
    };
    Ok(LoadedCredential {
        token,
        observation: CredentialObservation {
            credential_configured: true,
            credential_source: source,
        },
    })
}

fn parse_environment(value: OsString) -> Result<Zeroizing<String>, CredentialLoadError> {
    let value = value
        .into_string()
        .map_err(|_| CredentialLoadError::new("credential_invalid"))?;
    validate_token(&value)?;
    Ok(Zeroizing::new(value))
}

fn validate_token(value: &str) -> Result<(), CredentialLoadError> {
    if value.is_empty()
        || value.len() > MAX_TOKEN_BYTES
        || value.trim().is_empty()
        || value.bytes().any(|byte| matches!(byte, 0 | b'\r' | b'\n'))
    {
        Err(CredentialLoadError::new("credential_invalid"))
    } else {
        Ok(())
    }
}

fn read_file(
    path: &Path,
    expected_uid: u32,
) -> Result<Option<Zeroizing<String>>, CredentialLoadError> {
    let Some(parent) = path.parent() else {
        return Err(CredentialLoadError::new("credential_invalid"));
    };
    match std::fs::symlink_metadata(parent) {
        Ok(metadata) => validate_directory_metadata(&metadata, expected_uid)?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(CredentialLoadError::new("credential_invalid")),
    }

    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
    }
    let mut file = match options.open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) if error.raw_os_error() == Some(libc::ELOOP) => {
            return Err(CredentialLoadError::new("credential_symlink"));
        }
        Err(_) => return Err(CredentialLoadError::new("credential_invalid")),
    };
    validate_file_metadata(&file, expected_uid)?;
    let mut bytes = Vec::new();
    file.by_ref()
        .take(MAX_FILE_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| CredentialLoadError::new("credential_invalid"))?;
    if bytes.len() as u64 > MAX_FILE_BYTES {
        return Err(CredentialLoadError::new("credential_invalid"));
    }
    parse_file(bytes).map(Some)
}

#[cfg(unix)]
fn current_uid() -> u32 {
    unsafe { libc::geteuid() }
}

#[cfg(not(unix))]
fn current_uid() -> u32 {
    0
}

#[cfg(unix)]
fn validate_directory_metadata(
    metadata: &std::fs::Metadata,
    expected_uid: u32,
) -> Result<(), CredentialLoadError> {
    use std::os::unix::fs::MetadataExt;
    if metadata.file_type().is_symlink() || !metadata.file_type().is_dir() {
        return Err(CredentialLoadError::new("credential_symlink"));
    }
    if metadata.uid() != expected_uid {
        return Err(CredentialLoadError::new("credential_owner"));
    }
    if metadata.mode() & 0o777 != 0o700 {
        return Err(CredentialLoadError::new("credential_permission"));
    }
    Ok(())
}

#[cfg(not(unix))]
fn validate_directory_metadata(
    _metadata: &std::fs::Metadata,
    _expected_uid: u32,
) -> Result<(), CredentialLoadError> {
    Err(CredentialLoadError::new("credential_permission"))
}

#[cfg(unix)]
fn validate_file_metadata(file: &File, expected_uid: u32) -> Result<(), CredentialLoadError> {
    use std::os::unix::fs::MetadataExt;
    let metadata = file
        .metadata()
        .map_err(|_| CredentialLoadError::new("credential_invalid"))?;
    if !metadata.file_type().is_file() {
        return Err(CredentialLoadError::new("credential_invalid"));
    }
    if metadata.uid() != expected_uid {
        return Err(CredentialLoadError::new("credential_owner"));
    }
    if metadata.mode() & 0o777 != 0o600 {
        return Err(CredentialLoadError::new("credential_permission"));
    }
    Ok(())
}

#[cfg(not(unix))]
fn validate_file_metadata(_file: &File, _expected_uid: u32) -> Result<(), CredentialLoadError> {
    Err(CredentialLoadError::new("credential_permission"))
}

fn parse_file(mut bytes: Vec<u8>) -> Result<Zeroizing<String>, CredentialLoadError> {
    if bytes.last() == Some(&b'\n') {
        bytes.pop();
    }
    if bytes.is_empty() || bytes.iter().any(|byte| matches!(byte, 0 | b'\r' | b'\n')) {
        return Err(CredentialLoadError::new("credential_invalid"));
    }
    let value =
        String::from_utf8(bytes).map_err(|_| CredentialLoadError::new("credential_invalid"))?;
    let token = value
        .strip_prefix("LARM_API_TOKEN=")
        .ok_or_else(|| CredentialLoadError::new("credential_invalid"))?;
    validate_token(token)?;
    Ok(Zeroizing::new(token.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    #[cfg(unix)]
    use std::os::unix::fs::{symlink, PermissionsExt};
    use std::path::PathBuf;

    fn fixture() -> tempfile::TempDir {
        let home = tempfile::tempdir().expect("temporary home");
        let ssh = home.path().join(".ssh");
        fs::create_dir(&ssh).expect("ssh directory");
        #[cfg(unix)]
        fs::set_permissions(&ssh, fs::Permissions::from_mode(0o700)).expect("ssh permissions");
        home
    }

    fn write_credential(home: &Path, contents: &str, mode: u32) -> PathBuf {
        let path = home.join(CREDENTIAL_FILE);
        fs::write(&path, contents).expect("credential fixture");
        #[cfg(unix)]
        fs::set_permissions(&path, fs::Permissions::from_mode(mode))
            .expect("credential permissions");
        path
    }

    fn uid() -> u32 {
        current_uid()
    }

    #[test]
    fn loads_from_process_environment() {
        let home = fixture();
        let credential = load_from(Some("dummy-env-token".into()), home.path(), uid()).unwrap();
        assert_eq!(credential.token(), "dummy-env-token");
        assert_eq!(
            credential.observation().credential_source,
            CredentialSource::ProcessEnv
        );
    }

    #[test]
    fn loads_from_user_file_and_accepts_an_equal_environment_value() {
        let home = fixture();
        write_credential(home.path(), "LARM_API_TOKEN=dummy-file-token\n", 0o600);
        let file = load_from(None, home.path(), uid()).unwrap();
        assert_eq!(file.token(), "dummy-file-token");
        assert_eq!(
            file.observation().credential_source,
            CredentialSource::UserFile
        );
        let equal = load_from(Some("dummy-file-token".into()), home.path(), uid()).unwrap();
        assert_eq!(
            equal.observation().credential_source,
            CredentialSource::ProcessEnv
        );
    }

    #[test]
    fn rejects_conflicting_environment_and_file_values_without_exposing_them() {
        let home = fixture();
        write_credential(home.path(), "LARM_API_TOKEN=dummy-file-token\n", 0o600);
        let error = load_from(Some("dummy-env-token".into()), home.path(), uid()).unwrap_err();
        assert_eq!(error.code(), "credential_conflict");
        let debug = format!("{error:?}");
        assert!(!debug.contains("dummy-env-token"));
        assert!(!debug.contains("dummy-file-token"));
    }

    #[test]
    fn rejects_missing_empty_malformed_duplicate_and_unknown_keys() {
        let missing = fixture();
        let error = load_from(None, missing.path(), uid()).unwrap_err();
        assert_eq!(error.code(), "credential_missing");
        assert_eq!(
            error.observation().credential_source,
            CredentialSource::Missing
        );

        for contents in [
            "LARM_API_TOKEN=\n",
            "LARM_API_TOKEN=   \n",
            "not-an-assignment\n",
            "LARM_API_TOKEN=one\nLARM_API_TOKEN=two\n",
            "UNKNOWN=dummy\n",
            "LARM_API_TOKEN=dummy\nUNKNOWN=dummy\n",
            "# comment\nLARM_API_TOKEN=dummy\n",
            "\nLARM_API_TOKEN=dummy\n",
        ] {
            let home = fixture();
            write_credential(home.path(), contents, 0o600);
            assert_eq!(
                load_from(None, home.path(), uid()).unwrap_err().code(),
                "credential_invalid"
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn rejects_world_readable_file_and_symlink() {
        let readable = fixture();
        write_credential(readable.path(), "LARM_API_TOKEN=dummy\n", 0o644);
        assert_eq!(
            load_from(None, readable.path(), uid()).unwrap_err().code(),
            "credential_permission"
        );

        let linked = fixture();
        let target = linked.path().join("target");
        fs::write(&target, "LARM_API_TOKEN=dummy\n").unwrap();
        fs::set_permissions(&target, fs::Permissions::from_mode(0o600)).unwrap();
        symlink(&target, linked.path().join(CREDENTIAL_FILE)).unwrap();
        assert_eq!(
            load_from(None, linked.path(), uid()).unwrap_err().code(),
            "credential_symlink"
        );
    }

    #[cfg(unix)]
    #[test]
    fn rejects_owner_mismatch() {
        let home = fixture();
        write_credential(home.path(), "LARM_API_TOKEN=dummy\n", 0o600);
        assert_eq!(
            load_from(None, home.path(), uid().wrapping_add(1))
                .unwrap_err()
                .code(),
            "credential_owner"
        );
    }

    #[test]
    fn debug_output_contains_only_the_allowed_observation() {
        let home = fixture();
        let loaded = load_from(Some("dummy-secret-token".into()), home.path(), uid()).unwrap();
        let debug = format!("{loaded:?}");
        assert_eq!(
            debug,
            "LoadedCredential { credentialConfigured: true, credentialSource: \"process-env\" }"
        );
        assert!(!debug.contains("dummy-secret-token"));
    }
}
