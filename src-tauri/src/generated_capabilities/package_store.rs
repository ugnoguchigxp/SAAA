use serde_json::Value;
use std::{
    fs,
    io::Read,
    path::{Path, PathBuf},
};

use super::{
    contracts::{self, is_hash, package_hash, safe_flat_path, sha256_hex, PackageManifest},
    errors::*,
    limits,
};

/// Managed package area rooted at `<data directory>/generated-capabilities`.
#[derive(Clone, Debug)]
pub struct PackageStore {
    root: PathBuf,
}

#[derive(Clone, Debug)]
pub struct StagedPackage {
    pub staging_dir: PathBuf,
    pub manifest: PackageManifest,
    pub manifest_value: Value,
    pub package_hash: String,
    pub inventory: Vec<(String, String)>,
}

impl PackageStore {
    /// The managed layout is created lazily so that constructing a disabled service (for
    /// example in tests) has no filesystem side effects.
    pub fn open(data_directory: &Path) -> Self {
        Self {
            root: data_directory.join("generated-capabilities"),
        }
    }

    pub fn ensure_layout(&self) {
        for child in ["staging", "packages", "reports"] {
            let _ = fs::create_dir_all(self.root.join(child));
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn package_dir(&self, package_hash: &str) -> PathBuf {
        self.root.join("packages").join(package_hash)
    }

    pub fn manifest_path(&self, package_hash: &str) -> PathBuf {
        self.package_dir(package_hash).join("capability.json")
    }

    pub fn report_path(&self, check_id: &str) -> PathBuf {
        self.ensure_layout();
        self.root.join("reports").join(format!("{check_id}.json"))
    }

    pub fn create_staging(&self, import_id: &str) -> CapabilityResult<PathBuf> {
        self.ensure_layout();
        let staging = self.root.join("staging").join(import_id);
        fs::create_dir(&staging).map_err(|_| {
            CapabilityError::new(
                CapabilityErrorCode::StorageError,
                "could not create the staging directory",
            )
        })?;
        Ok(staging)
    }

    pub fn discard_staging(&self, staging: &Path) {
        let _ = fs::remove_dir_all(staging);
    }

    /// Copies the manifest and exactly its five referenced files into staging. Extra files in
    /// the candidate directory (including JS or a runtime directory) are never copied.
    pub fn stage(
        &self,
        candidate_directory: &Path,
        staging: &Path,
    ) -> CapabilityResult<StagedPackage> {
        let manifest_path = candidate_directory.join("capability.json");
        let bytes = read_regular_bounded(&manifest_path, limits::MAX_MANIFEST_BYTES as u64)
            .map_err(|error| map_layout(error, "candidate manifest is unreadable"))?;
        let manifest_value: Value = serde_json::from_slice(&bytes).map_err(|_| {
            CapabilityError::new(
                CapabilityErrorCode::InvalidPackage,
                "candidate manifest is not valid JSON",
            )
        })?;
        let manifest: PackageManifest =
            serde_json::from_value(manifest_value.clone()).map_err(|_| {
                CapabilityError::new(
                    CapabilityErrorCode::InvalidPackage,
                    "candidate manifest does not match the v2 contract",
                )
            })?;
        manifest
            .validate()
            .map_err(|message| CapabilityError::new(manifest_error_code(&message), message))?;

        let mut total = bytes.len() as u64;
        let mut inventory: Vec<(String, String)> =
            vec![("capability.json".to_string(), sha256_hex(&bytes))];
        fs::write(staging.join("capability.json"), &bytes).map_err(|_| {
            CapabilityError::new(
                CapabilityErrorCode::StorageError,
                "could not write the staged manifest",
            )
        })?;
        for (role, file) in manifest.files.entries() {
            if !safe_flat_path(&file.path) {
                return error(
                    CapabilityErrorCode::UnsupportedPackageLayout,
                    format!("package role {role} does not use a flat file name"),
                );
            }
            let source = candidate_directory.join(&file.path);
            let content = read_regular_bounded(&source, limits::MAX_CANDIDATE_FILE_BYTES)
                .map_err(|error| map_layout(error, "candidate file is unreadable"))?;
            if sha256_hex(&content) != file.hash {
                return error(
                    CapabilityErrorCode::IntegrityError,
                    format!("package role {role} hash mismatch"),
                );
            }
            total += content.len() as u64;
            if total > limits::MAX_CANDIDATE_TOTAL_BYTES {
                return error(
                    CapabilityErrorCode::InvalidPackage,
                    "candidate package exceeds the total size limit",
                );
            }
            fs::write(staging.join(&file.path), &content).map_err(|_| {
                CapabilityError::new(
                    CapabilityErrorCode::StorageError,
                    "could not write a staged package file",
                )
            })?;
            inventory.push((file.path.clone(), file.hash.clone()));
        }
        inventory.sort_by(|left, right| left.0.cmp(&right.0));
        inventory.dedup_by(|left, right| left.0 == right.0);
        if inventory.len() != limits::MAX_CANDIDATE_FILES {
            return error(
                CapabilityErrorCode::InvalidPackage,
                "candidate package does not contain exactly six files",
            );
        }
        let package_hash = package_hash(&manifest_value);
        if !is_hash(&package_hash) {
            return error(
                CapabilityErrorCode::InvalidPackage,
                "candidate package hash is invalid",
            );
        }
        Ok(StagedPackage {
            staging_dir: staging.to_path_buf(),
            manifest,
            manifest_value,
            package_hash,
            inventory,
        })
    }

    /// Atomically moves staging into `packages/<package_hash>`. An existing directory is never
    /// overwritten: it is compared and only shared when every byte matches.
    pub fn finalize(&self, staged: &StagedPackage) -> CapabilityResult<PathBuf> {
        self.ensure_layout();
        let target = self.package_dir(&staged.package_hash);
        if target.exists() {
            self.verify_copy(&staged.package_hash, &staged.inventory)?;
            self.discard_staging(&staged.staging_dir);
            return Ok(target);
        }
        fs::rename(&staged.staging_dir, &target).map_err(|_| {
            CapabilityError::new(
                CapabilityErrorCode::StorageError,
                "could not publish the staged package",
            )
        })?;
        Ok(target)
    }

    /// Re-checks the managed copy against the recorded inventory.
    pub fn verify_copy(
        &self,
        package_hash: &str,
        inventory: &[(String, String)],
    ) -> CapabilityResult<()> {
        if !is_hash(package_hash) {
            return error(
                CapabilityErrorCode::IntegrityError,
                "managed package hash is invalid",
            );
        }
        let directory = self.package_dir(package_hash);
        let mut present = Vec::new();
        let entries = fs::read_dir(&directory).map_err(|_| {
            CapabilityError::new(
                CapabilityErrorCode::IntegrityError,
                "managed package is missing",
            )
        })?;
        for entry in entries {
            let entry = entry.map_err(|_| {
                CapabilityError::new(
                    CapabilityErrorCode::IntegrityError,
                    "managed package is unreadable",
                )
            })?;
            let name = entry.file_name().to_string_lossy().into_owned();
            let content =
                read_regular_bounded(&directory.join(&name), limits::MAX_CANDIDATE_FILE_BYTES)
                    .map_err(|_| {
                        CapabilityError::new(
                            CapabilityErrorCode::IntegrityError,
                            "managed package file is unreadable",
                        )
                    })?;
            present.push((name, sha256_hex(&content)));
        }
        present.sort_by(|left, right| left.0.cmp(&right.0));
        if present != inventory {
            return error(
                CapabilityErrorCode::IntegrityError,
                "managed package contents changed",
            );
        }
        Ok(())
    }

    /// Lists package directories that have no catalog row (orphans from an interrupted import).
    pub fn orphan_packages(&self, known: &[String]) -> CapabilityResult<Vec<String>> {
        self.ensure_layout();
        let mut orphans = Vec::new();
        let entries = fs::read_dir(self.root.join("packages")).map_err(|_| {
            CapabilityError::new(
                CapabilityErrorCode::StorageError,
                "managed package area is unreadable",
            )
        })?;
        for entry in entries {
            let entry = entry.map_err(|_| {
                CapabilityError::new(
                    CapabilityErrorCode::StorageError,
                    "managed package area is unreadable",
                )
            })?;
            let name = entry.file_name().to_string_lossy().into_owned();
            if entry.path().is_dir() && !known.iter().any(|hash| hash == &name) {
                orphans.push(name);
            }
        }
        orphans.sort();
        Ok(orphans)
    }

    pub fn package_inventory(&self, package_hash: &str) -> CapabilityResult<Vec<(String, String)>> {
        let directory = self.package_dir(package_hash);
        let mut inventory = Vec::new();
        let entries = fs::read_dir(&directory).map_err(|_| {
            CapabilityError::new(
                CapabilityErrorCode::IntegrityError,
                "managed package is missing",
            )
        })?;
        for entry in entries {
            let entry = entry.map_err(|_| {
                CapabilityError::new(
                    CapabilityErrorCode::IntegrityError,
                    "managed package is unreadable",
                )
            })?;
            let name = entry.file_name().to_string_lossy().into_owned();
            let content =
                read_regular_bounded(&directory.join(&name), limits::MAX_CANDIDATE_FILE_BYTES)
                    .map_err(|_| {
                        CapabilityError::new(
                            CapabilityErrorCode::IntegrityError,
                            "managed package file is unreadable",
                        )
                    })?;
            inventory.push((name, sha256_hex(&content)));
        }
        inventory.sort_by(|left, right| left.0.cmp(&right.0));
        Ok(inventory)
    }

    pub fn inventory_hash(inventory: &[(String, String)]) -> String {
        contracts::inventory_hash(inventory)
    }
}

fn read_regular_bounded(path: &Path, limit: u64) -> CapabilityResult<Vec<u8>> {
    let metadata = fs::symlink_metadata(path).map_err(|_| {
        CapabilityError::new(
            CapabilityErrorCode::UnsupportedPackageLayout,
            "expected a regular file",
        )
    })?;
    if !metadata.file_type().is_file() || metadata.file_type().is_symlink() {
        return error(
            CapabilityErrorCode::UnsupportedPackageLayout,
            "package files must be regular non-symlink files",
        );
    }
    if metadata.len() > limit {
        return error(
            CapabilityErrorCode::InvalidPackage,
            "package file exceeds its size limit",
        );
    }
    let mut file = fs::File::open(path).map_err(|_| {
        CapabilityError::new(
            CapabilityErrorCode::UnsupportedPackageLayout,
            "package file is unreadable",
        )
    })?;
    let mut buffer = Vec::new();
    file.by_ref()
        .take(limit + 1)
        .read_to_end(&mut buffer)
        .map_err(|_| {
            CapabilityError::new(
                CapabilityErrorCode::UnsupportedPackageLayout,
                "package file is unreadable",
            )
        })?;
    if buffer.len() as u64 > limit {
        return error(
            CapabilityErrorCode::InvalidPackage,
            "package file exceeds its size limit",
        );
    }
    Ok(buffer)
}

fn map_layout(error: CapabilityError, message: &str) -> CapabilityError {
    CapabilityError::new(error.code, message)
}

fn manifest_error_code(message: &str) -> CapabilityErrorCode {
    if message.contains("file reference") {
        CapabilityErrorCode::UnsupportedPackageLayout
    } else if message.contains("unsupported capability manifest") {
        CapabilityErrorCode::UnsupportedContract
    } else {
        CapabilityErrorCode::InvalidPackage
    }
}
