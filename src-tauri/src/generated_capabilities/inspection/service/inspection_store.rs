use super::*;
pub const INSPECTION_DIRECTORY: &str = "generated-inspections";
pub const TYPESCRIPT_FILE: &str = "program.inspection.ts";
pub const REPORT_FILE: &str = "inspection.json";
/// Runs the fixed L-Lang inspection for one package directory. Implementations must run the
/// trusted kit, never candidate JavaScript.
pub trait Inspector {
    fn inspect(
        &self,
        package_directory: &Path,
        package_hash: &str,
    ) -> InspectionResult<InspectionReport>;
}
#[derive(Clone, Debug)]
pub struct InspectionStore {
    pub(super) root: PathBuf,
}
impl InspectionStore {
    pub fn open(data_directory: &Path) -> Self {
        Self {
            root: data_directory.join(INSPECTION_DIRECTORY),
        }
    }

    pub fn ensure_layout(&self) -> InspectionResult<()> {
        fs::create_dir_all(&self.root).map_err(|_| {
            InspectionError::new(
                InspectionErrorCode::Storage,
                "could not create the inspection area",
            )
        })
    }

    pub fn directory(&self, inspection_id: &str) -> PathBuf {
        self.root.join(inspection_id)
    }

    pub fn staging(&self, inspection_id: &str) -> PathBuf {
        self.root.join(format!(".{inspection_id}.pending"))
    }

    /// Writes both artifacts into a staging directory, then renames it into place. The rename is
    /// the only publication step; the caller records the DB row afterwards.
    pub fn publish(
        &self,
        inspection_id: &str,
        typescript: &str,
        report_bytes: &[u8],
    ) -> InspectionResult<String> {
        self.ensure_layout()?;
        let staging = self.staging(inspection_id);
        let _ = fs::remove_dir_all(&staging);
        fs::create_dir(&staging).map_err(|_| {
            InspectionError::new(
                InspectionErrorCode::Storage,
                "could not create inspection staging",
            )
        })?;
        let write = |name: &str, bytes: &[u8]| -> InspectionResult<()> {
            fs::write(staging.join(name), bytes).map_err(|_| {
                InspectionError::new(
                    InspectionErrorCode::Storage,
                    "could not write an inspection artifact",
                )
            })
        };
        let result = write(TYPESCRIPT_FILE, typescript.as_bytes())
            .and_then(|()| write(REPORT_FILE, report_bytes));
        if let Err(error) = result {
            let _ = fs::remove_dir_all(&staging);
            return Err(error);
        }
        let target = self.directory(inspection_id);
        if target.exists() {
            let _ = fs::remove_dir_all(&staging);
            return Err(InspectionError::new(
                InspectionErrorCode::Storage,
                "inspection directory already exists",
            ));
        }
        fs::rename(&staging, &target).map_err(|_| {
            let _ = fs::remove_dir_all(&staging);
            InspectionError::new(
                InspectionErrorCode::Storage,
                "could not publish the inspection directory",
            )
        })?;
        Ok(inspection_id.to_string())
    }

    pub fn discard(&self, inspection_id: &str) {
        let _ = fs::remove_dir_all(self.staging(inspection_id));
    }

    pub fn read_typescript(&self, inspection_id: &str) -> InspectionResult<String> {
        fs::read_to_string(self.directory(inspection_id).join(TYPESCRIPT_FILE)).map_err(|_| {
            InspectionError::new(
                InspectionErrorCode::ArtifactMissing,
                "the stored inspection TypeScript is missing",
            )
        })
    }

    pub fn read_report(&self, inspection_id: &str) -> InspectionResult<Vec<u8>> {
        fs::read(self.directory(inspection_id).join(REPORT_FILE)).map_err(|_| {
            InspectionError::new(
                InspectionErrorCode::ArtifactMissing,
                "the stored inspection report is missing",
            )
        })
    }

    /// Startup recovery: directories with no DB row are removed. No arbitrary path is followed.
    pub fn orphan_directories(&self, known: &[String]) -> InspectionResult<Vec<String>> {
        self.ensure_layout()?;
        let entries = fs::read_dir(&self.root).map_err(|_| {
            InspectionError::new(
                InspectionErrorCode::Storage,
                "inspection area is unreadable",
            )
        })?;
        let mut orphans = Vec::new();
        for entry in entries {
            let entry = entry.map_err(|_| {
                InspectionError::new(
                    InspectionErrorCode::Storage,
                    "inspection area is unreadable",
                )
            })?;
            let name = entry.file_name().to_string_lossy().into_owned();
            if !entry.path().is_dir() || known.iter().any(|known| known == &name) {
                continue;
            }
            let path = entry.path();
            fs::remove_dir_all(&path).map_err(|_| {
                InspectionError::new(
                    InspectionErrorCode::Storage,
                    "could not remove an orphan inspection directory",
                )
            })?;
            orphans.push(name);
        }
        orphans.sort();
        Ok(orphans)
    }
}
struct ResolvedCall {
    pub(super) revision: repository::RevisionRow,
    pub(super) package_hash: String,
}
pub struct InspectionService {
    pub(super) store: InspectionStore,
    pub(super) packages: PackageStore,
    pub(super) inspector_digest: String,
}
impl InspectionService {
    pub fn new(data_directory: &Path, inspector_digest: String) -> Self {
        Self {
            store: InspectionStore::open(data_directory),
            packages: PackageStore::open(data_directory),
            inspector_digest,
        }
    }

    pub fn store(&self) -> &InspectionStore {
        &self.store
    }

    pub fn inspect_execution(
        &self,
        writer: &SqliteWriter,
        context: &InspectionContext,
        call_id: &str,
        inspector: &dyn Inspector,
        projection: &dyn CaseEvaluator,
        wasm: &dyn CaseEvaluator,
    ) -> InspectionResult<InspectionReceipt> {
        let resolved = self.resolve(writer, context, call_id)?;
        // Existing evidence for the same (revision, inspector digest) is reused, never replaced.
        let existing = lifecycle::read(writer, |connection| {
            inspection_repository::by_revision_and_digest(
                connection,
                &resolved.revision.id,
                &self.inspector_digest,
            )
        })
        .map_err(to_inspection_error)?;
        if let Some(row) = existing {
            return self.load_receipt(row);
        }

        let manifest_path = self.packages.manifest_path(&resolved.package_hash);
        let report = inspector.inspect(
            manifest_path.parent().unwrap_or_else(|| Path::new(".")),
            &resolved.package_hash,
        )?;
        report.validate()?;
        let source_hash = resolved
            .revision
            .source_hash
            .as_deref()
            .ok_or_else(|| missing_artifact("revision has no recorded source hash"))?;
        let program_hash = resolved
            .revision
            .program_hash
            .as_deref()
            .ok_or_else(|| missing_artifact("revision has no recorded program hash"))?;
        let artifact_hash = resolved
            .revision
            .artifact_hash
            .as_deref()
            .ok_or_else(|| missing_artifact("revision has no recorded artifact hash"))?;
        report.matches_revision(
            &resolved.package_hash,
            source_hash,
            program_hash,
            artifact_hash,
        )?;
        let report_contract_hash =
            crate::generated_capabilities::contracts::contract_hash(&report.contract.input);
        if report_contract_hash != resolved.revision.contract_hash {
            return Err(InspectionError::new(
                InspectionErrorCode::Integrity,
                "the inspected contract disagrees with the executed revision",
            ));
        }

        let fields = report
            .contract
            .input
            .fields
            .iter()
            .map(|field| field.name.clone())
            .collect::<Vec<_>>();
        let comparison = compare(
            &fields,
            projection,
            wasm,
            &report.typescript.projection_hash,
            &report.artifacts.artifact_hash,
        )?;
        if !comparison.is_match() {
            return Err(InspectionError::new(
                InspectionErrorCode::Integrity,
                "the projection and Wasm disagree on the comparison domain",
            ));
        }
        self.publish(writer, &resolved, &report, comparison)
    }

    fn resolve(
        &self,
        writer: &SqliteWriter,
        context: &InspectionContext,
        call_id: &str,
    ) -> InspectionResult<ResolvedCall> {
        let owner = lifecycle::read(writer, |connection| {
            generation_repository::call_owner(connection, call_id)
        })
        .map_err(to_inspection_error)?;
        let owner = owner.ok_or_else(|| {
            // A call with no recorded owner is never attributed to the current user.
            InspectionError::new(
                InspectionErrorCode::NotAuthorized,
                "the call has no recorded owner",
            )
        })?;
        if owner.principal_id != context.principal_id {
            return Err(InspectionError::new(
                InspectionErrorCode::NotAuthorized,
                "the call belongs to another principal",
            ));
        }
        if let Some(project_id) = owner.project_id.as_deref() {
            if context.project_id.as_deref() != Some(project_id) {
                return Err(InspectionError::new(
                    InspectionErrorCode::NotAuthorized,
                    "the call project does not match the inspection context",
                ));
            }
        }

        let resolved = lifecycle::read(writer, |connection| {
            let call =
                generation_repository::call_by_id(connection, call_id)?.ok_or_else(|| {
                    CapabilityError::new(CapabilityErrorCode::NotValidated, "unknown call")
                })?;
            let revision = repository::revision_by_id(connection, &call.revision_id)?;
            if revision.package_hash != call.package_hash {
                return Err(CapabilityError::new(
                    CapabilityErrorCode::IntegrityError,
                    "call package hash disagrees with the revision",
                ));
            }
            Ok(ResolvedCall {
                revision,
                package_hash: call.package_hash,
            })
        })
        .map_err(|error| match error.code {
            CapabilityErrorCode::NotValidated => InspectionError::new(
                InspectionErrorCode::NotGenerated,
                "the call does not resolve to a generated revision",
            ),
            CapabilityErrorCode::IntegrityError => InspectionError::new(
                InspectionErrorCode::Integrity,
                "the call package hash disagrees with the revision",
            ),
            _ => to_inspection_error(error),
        })?;

        let inventory = self
            .packages
            .package_inventory(&resolved.package_hash)
            .map_err(|_| missing_artifact("the managed package is missing"))?;
        if PackageStore::inventory_hash(&inventory) != resolved.revision.inventory_hash {
            return Err(InspectionError::new(
                InspectionErrorCode::Integrity,
                "the managed package changed since verification",
            ));
        }
        Ok(resolved)
    }

    fn publish(
        &self,
        writer: &SqliteWriter,
        resolved: &ResolvedCall,
        report: &InspectionReport,
        comparison: ComparisonResult,
    ) -> InspectionResult<InspectionReceipt> {
        let inspection_id = crate::new_id("gcinspect");
        let report_value = serde_json::to_value(report).map_err(|_| {
            InspectionError::new(InspectionErrorCode::Storage, "report is not serialisable")
        })?;
        let report_bytes = serde_json::to_vec_pretty(report).map_err(|_| {
            InspectionError::new(InspectionErrorCode::Storage, "report is not serialisable")
        })?;
        if report.typescript.source.len() as u64 + report_bytes.len() as u64
            > super::super::contracts::MAX_INSPECTION_BYTES
        {
            return Err(InspectionError::new(
                InspectionErrorCode::InvalidInput,
                "inspection artifacts exceed 1 MiB",
            ));
        }
        self.store
            .publish(&inspection_id, &report.typescript.source, &report_bytes)?;
        let row = inspection_repository::NewInspection {
            id: inspection_id.clone(),
            revision_id: resolved.revision.id.clone(),
            inspector_digest: self.inspector_digest.clone(),
            package_hash: resolved.package_hash.clone(),
            source_hash: report.artifacts.source_hash.clone(),
            program_hash: report.artifacts.program_hash.clone(),
            artifact_hash: report.artifacts.artifact_hash.clone(),
            projection_hash: report.typescript.projection_hash.clone(),
            relative_directory: inspection_id.clone(),
            comparison_json: comparison.to_json().to_string(),
            created_at: generation_repository::unix_ms(),
        };
        let inserted = writer
            .write(|connection| {
                inspection_repository::insert(connection, &row).map_err(|error| error.encode())
            })
            .map_err(|encoded| to_inspection_error(CapabilityError::decode(encoded)));
        if let Err(error) = inserted {
            // The DB row is the source of truth; a directory with no row is an orphan removed at
            // startup, so it is discarded now as well.
            self.store.discard(&inspection_id);
            return Err(error);
        }
        Ok(InspectionReceipt {
            inspection_id,
            revision_id: resolved.revision.id.clone(),
            source_hash: report.artifacts.source_hash.clone(),
            program_hash: report.artifacts.program_hash.clone(),
            artifact_hash: report.artifacts.artifact_hash.clone(),
            projection_hash: report.typescript.projection_hash.clone(),
            typescript_text: report.typescript.source.clone(),
            report_json: report_value,
            comparison_json: comparison.to_json(),
        })
    }

    fn load_receipt(
        &self,
        row: inspection_repository::InspectionRow,
    ) -> InspectionResult<InspectionReceipt> {
        let typescript_text = self.store.read_typescript(&row.relative_directory)?;
        let report_bytes = self.store.read_report(&row.relative_directory)?;
        let report_json: serde_json::Value =
            serde_json::from_slice(&report_bytes).map_err(|_| {
                InspectionError::new(
                    InspectionErrorCode::Integrity,
                    "the stored inspection report is unreadable",
                )
            })?;
        let comparison_json: serde_json::Value = serde_json::from_str(&row.comparison_json)
            .map_err(|_| {
                InspectionError::new(
                    InspectionErrorCode::Integrity,
                    "the stored comparison is unreadable",
                )
            })?;
        let receipt = InspectionReceipt {
            inspection_id: row.id,
            revision_id: row.revision_id,
            source_hash: row.source_hash,
            program_hash: row.program_hash,
            artifact_hash: row.artifact_hash,
            projection_hash: row.projection_hash,
            typescript_text,
            report_json,
            comparison_json,
        };
        if !receipt.within_budget() {
            return Err(InspectionError::new(
                InspectionErrorCode::Integrity,
                "the stored inspection artifacts exceed 1 MiB",
            ));
        }
        Ok(receipt)
    }
}
pub(super) fn missing_artifact(message: &'static str) -> InspectionError {
    InspectionError::new(InspectionErrorCode::ArtifactMissing, message)
}
pub(super) fn to_inspection_error(error: CapabilityError) -> InspectionError {
    let code = match error.code {
        CapabilityErrorCode::NotActive => InspectionErrorCode::NotAuthorized,
        CapabilityErrorCode::Conflict => InspectionErrorCode::Conflict,
        CapabilityErrorCode::NotValidated => InspectionErrorCode::NotGenerated,
        CapabilityErrorCode::IntegrityError => InspectionErrorCode::Integrity,
        CapabilityErrorCode::Timeout => InspectionErrorCode::Timeout,
        CapabilityErrorCode::Cancelled => InspectionErrorCode::Cancelled,
        CapabilityErrorCode::Unavailable | CapabilityErrorCode::Disabled => {
            InspectionErrorCode::Unavailable
        }
        CapabilityErrorCode::InvalidInput => InspectionErrorCode::InvalidInput,
        _ => InspectionErrorCode::Storage,
    };
    InspectionError::new(code, "inspection failed")
}
