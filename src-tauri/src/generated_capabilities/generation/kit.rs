//! Trusted generation kit: bundle verification and the fixed package/inspect runner (plan 12.1).
//!
//! The kit is a self-contained Bun bundle produced by `scripts/llang/build-generation-kit.ts`.
//! The administrator-configured digest is the trust anchor; a freshly computed digest is never
//! adopted. The process is started with a cleared environment so provider credentials never reach
//! build or inspection.

use serde::Deserialize;
use std::{
    collections::BTreeMap,
    fs,
    io::Read,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant},
};

use super::super::contracts::sha256_hex;
use super::super::errors::{error, CapabilityErrorCode, CapabilityResult};
use super::config::GenerationConfig;
use super::contracts::GenerationErrorCode;

pub const KIT_FORMAT_VERSION: u32 = 1;
pub const PACKAGE_TIMEOUT: Duration = Duration::from_secs(120);
pub const INSPECT_TIMEOUT: Duration = Duration::from_secs(15);
pub const MAX_KIT_STDOUT_BYTES: usize = 1024 * 1024;
pub const MAX_KIT_STDERR_BYTES: usize = 64 * 1024;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct KitManifest {
    pub(super) format_version: u32,
    pub(super) entrypoint: String,
    pub(super) commands: KitCommands,
    pub(super) bun_version: String,
    pub(super) llang_version: Option<String>,
    pub(super) llang_dirty: Option<bool>,
    pub(super) files: BTreeMap<String, String>,
    pub(super) digest: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct KitCommands {
    pub(super) package: String,
    pub(super) inspect: String,
}

#[derive(Clone, Debug)]
pub struct GenerationKit {
    pub(super) root: PathBuf,
    pub(super) entrypoint: PathBuf,
    pub(super) bun_path: PathBuf,
    pub digest: String,
    pub bun_version: String,
    pub llang_version: String,
    pub llang_dirty: bool,
    pub(super) files: BTreeMap<String, String>,
}

#[derive(Debug)]
pub struct KitOutput {
    pub status: std::process::ExitStatus,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

impl GenerationKit {
    pub fn load(config: &GenerationConfig) -> CapabilityResult<Self> {
        let root = config.kit_root.canonicalize().map_err(|_| {
            super::contracts::encode_error(
                GenerationErrorCode::Unavailable,
                "the generation kit root is unavailable",
            )
        })?;
        let manifest_bytes = read_regular(&root.join("manifest.json"))?;
        let manifest: KitManifest = serde_json::from_slice(&manifest_bytes).map_err(|_| {
            super::contracts::encode_error(
                GenerationErrorCode::Unavailable,
                "the generation kit manifest is invalid",
            )
        })?;
        if manifest.format_version != KIT_FORMAT_VERSION || manifest.files.is_empty() {
            return Err(super::contracts::encode_error(
                GenerationErrorCode::Unavailable,
                "unsupported generation kit manifest",
            ));
        }
        if manifest.commands.package != "package" || manifest.commands.inspect != "inspect" {
            return Err(super::contracts::encode_error(
                GenerationErrorCode::Unavailable,
                "the generation kit does not declare the fixed package/inspect commands",
            ));
        }
        if !safe_relative(&manifest.entrypoint)
            || !manifest.files.contains_key(&manifest.entrypoint)
        {
            return Err(super::contracts::encode_error(
                GenerationErrorCode::Unavailable,
                "the generation kit entrypoint is not a listed file",
            ));
        }
        let digest = kit_digest(&manifest.files);
        if digest != manifest.digest || digest != config.expected_kit_digest {
            return Err(super::contracts::encode_error(
                GenerationErrorCode::Unavailable,
                "the generation kit digest does not match the configured trust value",
            ));
        }
        for (name, expected) in &manifest.files {
            if !safe_relative(name) {
                return Err(super::contracts::encode_error(
                    GenerationErrorCode::Unavailable,
                    "the generation kit lists an unsafe path",
                ));
            }
            if sha256_hex(&read_regular(&root.join(name))?) != *expected {
                return Err(super::contracts::encode_error(
                    GenerationErrorCode::Unavailable,
                    "a generation kit file changed",
                ));
            }
        }
        Ok(Self {
            entrypoint: root.join(&manifest.entrypoint),
            root,
            bun_path: config.bun_path.clone(),
            digest,
            bun_version: manifest.bun_version,
            llang_version: manifest.llang_version.unwrap_or_default(),
            llang_dirty: manifest.llang_dirty.unwrap_or(false),
            files: manifest.files,
        })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The trusted Bun executable. Used by inspection to evaluate the trusted projection in an
    /// isolated process; the candidate's own JavaScript is never an entrypoint.
    pub fn bun_path(&self) -> &Path {
        &self.bun_path
    }

    pub fn files(&self) -> &BTreeMap<String, String> {
        &self.files
    }

    /// Runs one kit command with a cleared environment and a bounded wall clock. The kit files are
    /// re-verified immediately before the spawn, stdout/stderr are drained concurrently so a full
    /// pipe cannot deadlock the child, and the child is killed when the deadline passes.
    pub fn run(
        &self,
        args: &[&str],
        timeout: Duration,
        working_directory: &Path,
    ) -> CapabilityResult<KitOutput> {
        self.run_script(&self.entrypoint, args, timeout, working_directory)
    }

    /// Runs an arbitrary trusted Bun script with the kit's Bun and the same environment, output and
    /// wall-clock limits as [`Self::run`]. Used to evaluate the inspector's own projection; the
    /// candidate's JavaScript is never passed here.
    pub fn run_script(
        &self,
        script: &Path,
        args: &[&str],
        timeout: Duration,
        working_directory: &Path,
    ) -> CapabilityResult<KitOutput> {
        self.revalidate(&self.digest)?;
        let path_var = std::env::var("PATH").unwrap_or_default();
        let mut command = Command::new(&self.bun_path);
        command
            .arg(script)
            .args(args)
            .current_dir(working_directory)
            .env_clear()
            .env("PATH", &path_var)
            .env("NO_COLOR", "1")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = command.spawn().map_err(|_| {
            super::contracts::encode_error(
                GenerationErrorCode::Unavailable,
                "the generation kit could not be started",
            )
        })?;
        // Drain both pipes on their own threads. Reading only after the child exits can deadlock
        // when the child fills a pipe buffer before it can exit.
        let stdout = child.stdout.take();
        let stderr = child.stderr.take();
        let stdout_handle = std::thread::spawn(move || drain(stdout, MAX_KIT_STDOUT_BYTES));
        let stderr_handle = std::thread::spawn(move || drain(stderr, MAX_KIT_STDERR_BYTES));
        let deadline = Instant::now() + timeout;
        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break Ok(status),
                Ok(None) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(20));
                }
                Ok(None) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    break Err(super::contracts::encode_error(
                        GenerationErrorCode::BudgetExceeded,
                        "the generation kit exceeded its wall-clock budget",
                    ));
                }
                Err(_) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    break Err(super::contracts::encode_error(
                        GenerationErrorCode::Unavailable,
                        "the generation kit could not be waited on",
                    ));
                }
            }
        };
        let (stdout, stdout_over) = stdout_handle.join().unwrap_or_default();
        let (stderr, stderr_over) = stderr_handle.join().unwrap_or_default();
        let status = status?;
        if stdout_over || stderr_over {
            return Err(super::contracts::encode_error(
                GenerationErrorCode::BudgetExceeded,
                "the generation kit output exceeded its limit",
            ));
        }
        Ok(KitOutput {
            status,
            stdout,
            stderr,
        })
    }

    /// Re-checks every kit file and the digest immediately before a run, so a kit changed after
    /// load is never executed.
    pub fn revalidate(&self, expected_digest: &str) -> CapabilityResult<()> {
        let digest = kit_digest(&self.files);
        if digest != expected_digest {
            return Err(super::contracts::encode_error(
                GenerationErrorCode::Unavailable,
                "the generation kit changed since it was trusted",
            ));
        }
        for (name, expected) in &self.files {
            if sha256_hex(&read_regular(&self.root.join(name))?) != *expected {
                return Err(super::contracts::encode_error(
                    GenerationErrorCode::Unavailable,
                    "a generation kit file changed",
                ));
            }
        }
        Ok(())
    }
}

/// Digest over the kit's fixed file list: sorted `name\0hash\n` records.
pub fn kit_digest(files: &BTreeMap<String, String>) -> String {
    let mut text = String::new();
    for (name, hash) in files {
        text.push_str(name);
        text.push('\u{0}');
        text.push_str(hash);
        text.push('\n');
    }
    sha256_hex(text.as_bytes())
}

/// Reads one pipe to EOF, keeping at most `limit` bytes and reporting whether the limit was hit.
fn drain<R: std::io::Read>(pipe: Option<R>, limit: usize) -> (Vec<u8>, bool) {
    let Some(pipe) = pipe else {
        return (Vec::new(), false);
    };
    let mut buffer = Vec::new();
    let mut limited = pipe.take(limit as u64 + 1);
    if limited.read_to_end(&mut buffer).is_err() {
        return (Vec::new(), true);
    }
    let exceeded = buffer.len() > limit;
    buffer.truncate(limit);
    (buffer, exceeded)
}

fn read_regular(path: &Path) -> CapabilityResult<Vec<u8>> {
    let metadata = fs::symlink_metadata(path).map_err(|_| {
        super::contracts::encode_error(
            GenerationErrorCode::Unavailable,
            "a generation kit file is unavailable",
        )
    })?;
    if !metadata.file_type().is_file() || metadata.file_type().is_symlink() {
        return error(
            CapabilityErrorCode::Unavailable,
            "generation kit files must be regular non-symlink files",
        );
    }
    fs::read(path).map_err(|_| {
        super::contracts::encode_error(
            GenerationErrorCode::Unavailable,
            "a generation kit file is unreadable",
        )
    })
}

fn safe_relative(value: &str) -> bool {
    let path = Path::new(value);
    !value.is_empty()
        && !path.is_absolute()
        && path
            .components()
            .all(|component| matches!(component, std::path::Component::Normal(_)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write(path: &Path, bytes: &[u8]) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        let mut file = fs::File::create(path).unwrap();
        file.write_all(bytes).unwrap();
    }

    fn kit_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "saaa-generation-kit-{tag}-{}",
            uuid::Uuid::new_v4().simple()
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn build_fake_kit(dir: &Path) -> GenerationConfig {
        write(&dir.join("llang-cli.js"), b"// kit entry\n");
        write(
            &dir.join("package.json"),
            b"{\"name\":\"kit\",\"private\":true}\n",
        );
        let mut files = BTreeMap::new();
        for name in ["llang-cli.js", "package.json"] {
            files.insert(
                name.to_string(),
                sha256_hex(&fs::read(dir.join(name)).unwrap()),
            );
        }
        let digest = kit_digest(&files);
        let manifest = serde_json::json!({
            "formatVersion": 1,
            "entrypoint": "llang-cli.js",
            "commands": { "package": "package", "inspect": "inspect" },
            "bunVersion": "1.4.2",
            "llangVersion": "abc",
            "llangDirty": false,
            "files": files,
            "digest": digest
        });
        write(&dir.join("manifest.json"), manifest.to_string().as_bytes());
        GenerationConfig {
            format_version: 1,
            enabled: true,
            bun_path: PathBuf::from("/bin/false"),
            kit_root: dir.to_path_buf(),
            expected_kit_digest: digest,
            requests_path: dir.join("requests.json"),
        }
    }

    #[test]
    fn kit_load_verifies_hashes_and_digest() {
        let dir = kit_dir("load");
        let config = build_fake_kit(&dir);
        let kit = GenerationKit::load(&config).unwrap();
        assert_eq!(kit.digest, config.expected_kit_digest);
        // A changed file is refused.
        write(&dir.join("llang-cli.js"), b"// changed\n");
        assert!(GenerationKit::load(&config).is_err());
    }

    #[test]
    fn drain_keeps_at_most_the_limit_and_reports_overflow() {
        let (bytes, overflow) = drain(Some(std::io::Cursor::new(vec![b'a'; 10])), 4);
        assert_eq!(bytes.len(), 4);
        assert!(overflow);
        let (bytes, overflow) = drain(Some(std::io::Cursor::new(b"abc".to_vec())), 4);
        assert_eq!(bytes, b"abc");
        assert!(!overflow);
        assert_eq!(drain::<std::io::Empty>(None, 4), (Vec::new(), false));
    }

    #[test]
    fn wrong_expected_digest_is_refused() {
        let dir = kit_dir("digest");
        let mut config = build_fake_kit(&dir);
        config.expected_kit_digest = "a".repeat(64);
        assert!(GenerationKit::load(&config).is_err());
    }
}
