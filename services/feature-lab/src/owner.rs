//! Exclusive owner for one lab database file. Released when the process drops it.
use fs2::FileExt;
use std::{
    fs::{File, OpenOptions},
    io,
    path::{Path, PathBuf},
};

pub struct OwnerGuard {
    _file: File,
}

pub fn acquire(database_path: &Path) -> Result<OwnerGuard, String> {
    let lock_path = lock_path(database_path)?;
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    let file = options
        .open(&lock_path)
        .map_err(|error| format!("feature lab ownership is unavailable: {error}"))?;
    validate_lock_file(&file)?;
    match file.try_lock_exclusive() {
        Ok(()) => Ok(OwnerGuard { _file: file }),
        Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
            Err("feature lab database is already owned".into())
        }
        Err(error) => Err(format!("feature lab ownership is unavailable: {error}")),
    }
}

fn lock_path(database_path: &Path) -> Result<PathBuf, String> {
    let parent = database_path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .ok_or_else(|| "feature lab database path has no parent directory".to_string())?;
    let name = database_path
        .file_name()
        .ok_or_else(|| "feature lab database path has no file name".to_string())?;
    Ok(parent.join(format!("{}.owner.lock", name.to_string_lossy())))
}

#[cfg(unix)]
fn validate_lock_file(file: &File) -> Result<(), String> {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};

    let metadata = file
        .metadata()
        .map_err(|error| format!("feature lab ownership is unavailable: {error}"))?;
    if !metadata.is_file()
        || metadata.nlink() != 1
        || metadata.permissions().mode() & 0o777 != 0o600
        || metadata.uid() != unsafe { libc::geteuid() }
    {
        return Err("feature lab ownership lock is not private".into());
    }
    Ok(())
}

#[cfg(not(unix))]
fn validate_lock_file(file: &File) -> Result<(), String> {
    if !file
        .metadata()
        .map_err(|error| format!("feature lab ownership is unavailable: {error}"))?
        .is_file()
    {
        return Err("feature lab ownership lock is not a regular file".into());
    }
    Ok(())
}
