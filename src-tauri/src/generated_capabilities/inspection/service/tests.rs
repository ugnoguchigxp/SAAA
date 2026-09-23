use super::*;
use serde_json::{Map, Value};
struct StaticInspector(InspectionReport);

impl Inspector for StaticInspector {
    fn inspect(
        &self,
        _package_directory: &Path,
        _package_hash: &str,
    ) -> InspectionResult<InspectionReport> {
        Ok(self.0.clone())
    }
}

struct ProductEval;

impl CaseEvaluator for ProductEval {
    fn evaluate(&self, input: &Map<String, Value>) -> Result<bool, String> {
        Ok(input.values().all(|value| value.as_bool() == Some(true)))
    }
}

struct AlwaysTruthy;

impl CaseEvaluator for AlwaysTruthy {
    fn evaluate(&self, _input: &Map<String, Value>) -> Result<bool, String> {
        Ok(true)
    }
}

fn sample_contract() -> crate::generated_capabilities::contracts::WasmContract {
    use crate::generated_capabilities::contracts::{ContractField, FieldKind, WasmContract};
    let field = |name: &str| ContractField {
        name: name.into(),
        kind: FieldKind::Boolean,
        values: vec![],
        nullable: false,
        undefinable: false,
        optional: false,
    };
    WasmContract {
        version: 1,
        fields: vec![field("enabled"), field("suspended")],
    }
}

fn report_for(enabled_only: bool) -> InspectionReport {
    let mut report = super::super::contracts::tests::sample_report();
    // Bind the sample report to the package hash and contract the test revision records.
    report.package_hash = "a".repeat(64);
    report.contract.input = sample_contract();
    if !enabled_only {
        report.typescript.source = "// enabled && !suspended".into();
    }
    report
}

fn test_writer() -> SqliteWriter {
    let connection = rusqlite::Connection::open_in_memory().unwrap();
    crate::persistence::schema::initialize_database(&connection).unwrap();
    SqliteWriter::from_connection(connection)
}

fn seed_call(writer: &SqliteWriter, data: &Path) {
    let store = PackageStore::open(data);
    store.ensure_layout();
    // Build a minimal managed package with the manifest only; the inventory hash must match
    // the revision row, so compute it from the same bytes.
    let package_hash = "a".repeat(64);
    let package_dir = store.package_dir(&package_hash);
    fs::create_dir_all(&package_dir).unwrap();
    fs::write(package_dir.join("capability.json"), b"{}").unwrap();
    let inventory = store.package_inventory(&package_hash).unwrap();
    let inventory_hash = PackageStore::inventory_hash(&inventory);
    writer
        .write(|connection| {
            connection
                .execute(
                    "INSERT INTO generated_capabilities(id, created_at, updated_at)
                         VALUES('c','x','x')",
                    [],
                )
                .unwrap();
            let contract = sample_contract();
            let contract_hash = crate::generated_capabilities::contracts::contract_hash(&contract);
            let contract_json = serde_json::to_string(&contract).unwrap();
            connection
                .execute(
                    "INSERT INTO generated_capability_revisions(
                           id, capability_id, package_hash, inventory_hash, contract_hash,
                           runtime_digest, required_acceptance_hash, provenance_json,
                           manifest_json, contract_json, metadata_json, state, source_hash,
                           program_hash, artifact_hash, created_at)
                         VALUES('r','c',?1,?2,?3,'d','a','{}','{}',?4,'{}','validated',
                                ?5, ?6, ?7, 'x')",
                    rusqlite::params![
                        package_hash,
                        inventory_hash,
                        contract_hash,
                        contract_json,
                        "d".repeat(64),
                        "e".repeat(64),
                        "f".repeat(64)
                    ],
                )
                .unwrap();
            connection
                .execute(
                    "INSERT INTO generated_capability_calls(
                           id, revision_id, package_hash, origin, status, started_at)
                         VALUES('call-1','r',?1,'conversation','succeeded','x')",
                    rusqlite::params![package_hash],
                )
                .unwrap();
            connection
                .execute(
                    "INSERT INTO generated_capability_call_owners(
                           call_id, principal_id, conversation_id, project_id, run_id)
                         VALUES('call-1','P1',?1,NULL,'run-1')",
                    rusqlite::params![crate::PRIMARY_CONVERSATION_ID],
                )
                .unwrap();
            Ok(())
        })
        .unwrap();
}

fn service(writer: &SqliteWriter, data: &Path) -> InspectionService {
    seed_call(writer, data);
    InspectionService::new(data, "9".repeat(64))
}

#[test]
fn inspection_publishes_and_reuses_evidence() {
    let data = tempfile::tempdir().unwrap();
    let writer = test_writer();
    let service = service(&writer, data.path());
    let context = InspectionContext {
        principal_id: "P1".into(),
        conversation_id: crate::PRIMARY_CONVERSATION_ID.into(),
        project_id: None,
    };
    let inspector = StaticInspector(report_for(true));
    let receipt = service
        .inspect_execution(
            &writer,
            &context,
            "call-1",
            &inspector,
            &ProductEval,
            &ProductEval,
        )
        .unwrap();
    assert!(receipt.typescript_text.contains("evaluate"));
    assert!(service
        .store()
        .read_typescript(&receipt.inspection_id)
        .is_ok());

    // A second inspection with the same inspector digest returns the same evidence.
    let again = service
        .inspect_execution(
            &writer,
            &context,
            "call-1",
            &inspector,
            &ProductEval,
            &ProductEval,
        )
        .unwrap();
    assert_eq!(receipt.inspection_id, again.inspection_id);
}

#[test]
fn another_principal_is_refused() {
    let data = tempfile::tempdir().unwrap();
    let writer = test_writer();
    let service = service(&writer, data.path());
    let context = InspectionContext {
        principal_id: "P2".into(),
        conversation_id: crate::PRIMARY_CONVERSATION_ID.into(),
        project_id: None,
    };
    let result = service.inspect_execution(
        &writer,
        &context,
        "call-1",
        &StaticInspector(report_for(true)),
        &ProductEval,
        &ProductEval,
    );
    assert_eq!(result.unwrap_err().code, InspectionErrorCode::NotAuthorized);
}

#[test]
fn a_project_scoped_call_requires_the_same_project() {
    let data = tempfile::tempdir().unwrap();
    let writer = test_writer();
    let service = service(&writer, data.path());
    // The seed owner has no project; record a project-scoped owner instead.
    writer
        .write(|connection| {
            connection
                .execute(
                    "UPDATE generated_capability_call_owners SET project_id = 'proj-A'
                         WHERE call_id = 'call-1'",
                    [],
                )
                .unwrap();
            Ok(())
        })
        .unwrap();
    let without_project = InspectionContext {
        principal_id: "P1".into(),
        conversation_id: crate::PRIMARY_CONVERSATION_ID.into(),
        project_id: None,
    };
    assert_eq!(
        service
            .inspect_execution(
                &writer,
                &without_project,
                "call-1",
                &StaticInspector(report_for(true)),
                &ProductEval,
                &ProductEval,
            )
            .unwrap_err()
            .code,
        InspectionErrorCode::NotAuthorized
    );
    let with_project = InspectionContext {
        principal_id: "P1".into(),
        conversation_id: crate::PRIMARY_CONVERSATION_ID.into(),
        project_id: Some("proj-A".into()),
    };
    assert!(service
        .inspect_execution(
            &writer,
            &with_project,
            "call-1",
            &StaticInspector(report_for(true)),
            &ProductEval,
            &ProductEval,
        )
        .is_ok());
}

#[test]
fn a_missing_stored_artifact_is_reported_not_regenerated() {
    let data = tempfile::tempdir().unwrap();
    let writer = test_writer();
    let service = service(&writer, data.path());
    let context = InspectionContext {
        principal_id: "P1".into(),
        conversation_id: crate::PRIMARY_CONVERSATION_ID.into(),
        project_id: None,
    };
    let receipt = service
        .inspect_execution(
            &writer,
            &context,
            "call-1",
            &StaticInspector(report_for(true)),
            &ProductEval,
            &ProductEval,
        )
        .unwrap();
    std::fs::remove_file(
        service
            .store()
            .directory(&receipt.inspection_id)
            .join(super::super::service::TYPESCRIPT_FILE),
    )
    .unwrap();
    let again = service.inspect_execution(
        &writer,
        &context,
        "call-1",
        &StaticInspector(report_for(true)),
        &ProductEval,
        &ProductEval,
    );
    assert_eq!(
        again.unwrap_err().code,
        InspectionErrorCode::ArtifactMissing
    );
}

#[test]
fn a_contract_mismatch_with_the_revision_is_refused() {
    let data = tempfile::tempdir().unwrap();
    let writer = test_writer();
    let service = service(&writer, data.path());
    let context = InspectionContext {
        principal_id: "P1".into(),
        conversation_id: crate::PRIMARY_CONVERSATION_ID.into(),
        project_id: None,
    };
    let mut report = report_for(true);
    report
        .contract
        .input
        .fields
        .push(crate::generated_capabilities::contracts::ContractField {
            name: "extra".into(),
            kind: crate::generated_capabilities::contracts::FieldKind::Boolean,
            values: vec![],
            nullable: false,
            undefinable: false,
            optional: false,
        });
    let result = service.inspect_execution(
        &writer,
        &context,
        "call-1",
        &StaticInspector(report),
        &ProductEval,
        &ProductEval,
    );
    assert_eq!(result.unwrap_err().code, InspectionErrorCode::Integrity);
}

#[test]
fn a_comparison_mismatch_is_never_published() {
    let data = tempfile::tempdir().unwrap();
    let writer = test_writer();
    let service = service(&writer, data.path());
    let context = InspectionContext {
        principal_id: "P1".into(),
        conversation_id: crate::PRIMARY_CONVERSATION_ID.into(),
        project_id: None,
    };
    let result = service.inspect_execution(
        &writer,
        &context,
        "call-1",
        &StaticInspector(report_for(false)),
        &ProductEval,
        &AlwaysTruthy,
    );
    assert_eq!(result.unwrap_err().code, InspectionErrorCode::Integrity);
    let known = lifecycle::read(&writer, inspection_repository::all_directories).unwrap();
    assert!(known.is_empty());
}
