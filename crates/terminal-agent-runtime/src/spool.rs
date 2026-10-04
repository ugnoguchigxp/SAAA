use fs2::FileExt;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Spec {
    pub job: String,
    pub run: String,
    pub nonce: String,
    pub workspace: PathBuf,
    pub executable: PathBuf,
    pub helper: PathBuf,
    pub cli: String,
    pub model: String,
    pub prompt: String,
    pub resume: Option<String>,
    pub answer: Option<Value>,
    pub deadline_seconds: u64,
    #[serde(default)]
    pub wake_path: Option<PathBuf>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Event {
    pub id: String,
    pub run: String,
    pub nonce: String,
    pub kind: String,
    pub data: Value,
}
fn err(e: impl std::fmt::Display) -> String {
    format!("terminal_storage: {e}")
}

pub fn write_private(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let temporary = path.with_extension(format!("tmp-{}", uuid::Uuid::new_v4().simple()));
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&temporary).map_err(err)?;
    file.write_all(bytes).map_err(err)?;
    file.sync_all().map_err(err)?;
    fs::rename(&temporary, path).map_err(err)?;
    fs::File::open(path.parent().ok_or("storage_parent_missing")?)
        .map_err(err)?
        .sync_all()
        .map_err(err)
}
pub fn create(directory: &Path, spec: &Spec) -> Result<(), String> {
    fs::create_dir_all(directory).map_err(err)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(directory, fs::Permissions::from_mode(0o700)).map_err(err)?;
    }
    write_private(
        &directory.join("spec.json"),
        &serde_json::to_vec(spec).map_err(err)?,
    )
}
pub fn read(directory: &Path) -> Result<Spec, String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let metadata = fs::symlink_metadata(directory).map_err(err)?;
        if !metadata.is_dir()
            || metadata.mode() & 0o077 != 0
            || metadata.uid() != unsafe { libc::getuid() }
        {
            return Err("terminal_directory_not_private".into());
        }
    }
    let file = directory.join("spec.json");
    if fs::symlink_metadata(&file)
        .map_err(err)?
        .file_type()
        .is_symlink()
        || fs::metadata(&file).map_err(err)?.len() > 128 * 1024
    {
        return Err("terminal_spec_invalid".into());
    }
    let spec: Spec = serde_json::from_slice(&fs::read(file).map_err(err)?).map_err(err)?;
    if !matches!(spec.cli.as_str(), "claude" | "codex")
        || spec.prompt.len() > 64000
        || spec.nonce.len() != 32
        || spec.deadline_seconds == 0
        || spec.deadline_seconds > 1800
    {
        return Err("terminal_spec_invalid".into());
    }
    Ok(spec)
}
pub fn append(directory: &Path, spec: &Spec, kind: &str, data: Value) -> Result<(), String> {
    let event = Event {
        id: uuid::Uuid::new_v4().simple().to_string(),
        run: spec.run.clone(),
        nonce: spec.nonce.clone(),
        kind: kind.into(),
        data,
    };
    let mut bytes = serde_json::to_vec(&event).map_err(err)?;
    if bytes.len() > 65536 {
        return Err("terminal_event_too_large".into());
    }
    bytes.push(b'\n');
    let mut options = OpenOptions::new();
    options.create(true).append(true).read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(directory.join("events.jsonl")).map_err(err)?;
    file.lock_exclusive().map_err(err)?;
    if file.metadata().map_err(err)?.len() + bytes.len() as u64 > 20 * 1024 * 1024 {
        return Err("terminal_event_budget_exhausted".into());
    }
    file.write_all(&bytes).map_err(err)?;
    file.sync_data().map_err(err)?;
    FileExt::unlock(&file).map_err(err)?;
    #[cfg(unix)]
    if let Some(path) = &spec.wake_path {
        if let Ok(socket) = std::os::unix::net::UnixDatagram::unbound() {
            let _ = socket.send_to(
                &serde_json::to_vec(&serde_json::json!({"run":spec.run,"nonce":spec.nonce}))
                    .map_err(err)?,
                path,
            );
        }
    }
    Ok(())
}
