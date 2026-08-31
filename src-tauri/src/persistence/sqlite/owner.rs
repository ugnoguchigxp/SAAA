use fs2::FileExt;
use std::{
    fs::{File, OpenOptions},
    io,
    path::{Path, PathBuf},
};

pub(super) struct DatabaseOwnerGuard {
    _file: File,
}

#[derive(Debug)]
pub(super) enum OwnershipError {
    AlreadyOwned,
    Unavailable(io::Error),
}

impl DatabaseOwnerGuard {
    pub(super) fn acquire(database_path: &Path) -> Result<Self, OwnershipError> {
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
            .map_err(OwnershipError::Unavailable)?;
        validate_lock_file(&file).map_err(OwnershipError::Unavailable)?;
        match file.try_lock_exclusive() {
            Ok(()) => Ok(Self { _file: file }),
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                Err(OwnershipError::AlreadyOwned)
            }
            Err(error) => Err(OwnershipError::Unavailable(error)),
        }
    }
}

fn lock_path(database_path: &Path) -> Result<PathBuf, OwnershipError> {
    let parent = database_path.parent().ok_or_else(|| {
        OwnershipError::Unavailable(io::Error::other("Database path has no parent directory"))
    })?;
    Ok(parent.join("saaa.sqlite3.writer.lock"))
}

#[cfg(unix)]
fn validate_lock_file(file: &File) -> io::Result<()> {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};

    let metadata = file.metadata()?;
    if !metadata.is_file()
        || metadata.nlink() != 1
        || metadata.permissions().mode() & 0o777 != 0o600
        || metadata.uid() != unsafe { libc::geteuid() }
    {
        return Err(io::Error::other("Database ownership lock is not private"));
    }
    Ok(())
}

#[cfg(not(unix))]
fn validate_lock_file(file: &File) -> io::Result<()> {
    if !file.metadata()?.is_file() {
        return Err(io::Error::other(
            "Database ownership lock is not a regular file",
        ));
    }
    Ok(())
}
