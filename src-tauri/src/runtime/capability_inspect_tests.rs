use super::*;
use crate::generated_capabilities::errors::CapabilityErrorCode;
use crate::generated_capabilities::generation::repository as generation_repository;
use crate::generated_capabilities::inspection::repository::{
    self as inspection_repository, NewInspection,
};
use crate::generated_capabilities::inspection::service::InspectionStore;
use crate::generated_capabilities::lifecycle;
use crate::persistence::schema::initialize_database;
use crate::persistence::SqliteWriter;
use serde_json::json;

fn hash() -> String {
    "a".repeat(64)
}

fn seed_call(writer: &SqliteWriter, principal: &str) {
    writer
        .write(|connection| {
            connection
                .execute(
                    "INSERT INTO generated_capabilities(id, created_at, updated_at)
                         VALUES('c','x','x')",
                    [],
                )
                .unwrap();
            connection
                .execute(
                    "INSERT INTO generated_capability_revisions(
                           id, capability_id, package_hash, inventory_hash, contract_hash,
                           runtime_digest, required_acceptance_hash, provenance_json,
                           manifest_json, contract_json, metadata_json, state, created_at)
                         VALUES('r','c','p','i','c','d','a','{}','{}','{}','{}','validated','x')",
                    [],
                )
                .unwrap();
            connection
                .execute(
                    "INSERT INTO generated_capability_calls(
                           id, revision_id, package_hash, origin, status, started_at)
                         VALUES('call-1','r','p','conversation','succeeded','x')",
                    [],
                )
                .unwrap();
            generation_repository::insert_call_owner(
                connection,
                "call-1",
                principal,
                crate::PRIMARY_CONVERSATION_ID,
                None,
                "run-1",
            )
            .unwrap();
            Ok(())
        })
        .unwrap();
}

#[test]
fn gc_01_stored_inspection_is_shown_and_foreign_calls_are_refused() {
    let data = tempfile::tempdir().unwrap();
    let connection = rusqlite::Connection::open_in_memory().unwrap();
    initialize_database(&connection).unwrap();
    let writer = SqliteWriter::from_connection(connection);
    seed_call(&writer, "P1");
    let ts = "export const marker = \"rev-A\";\n";
    let store = InspectionStore::open(data.path());
    store
        .publish(
            "insp-1",
            ts,
            &serde_json::to_vec(&json!({ "typescript": { "source": ts } })).unwrap(),
        )
        .unwrap();
    lifecycle::transaction(&writer, |transaction| {
        inspection_repository::insert(
            transaction,
            &NewInspection {
                id: "insp-1".into(),
                revision_id: "r".into(),
                inspector_digest: hash(),
                package_hash: hash(),
                source_hash: hash(),
                program_hash: hash(),
                artifact_hash: hash(),
                projection_hash: hash(),
                relative_directory: "insp-1".into(),
                comparison_json: "{}".into(),
                created_at: 1,
            },
        )
    })
    .unwrap();
    assert_eq!(
        load_stored_inspection(&writer, data.path(), "P1", "missing")
            .unwrap_err()
            .code,
        CapabilityErrorCode::NotActive
    );
    let shown = load_stored_inspection(&writer, data.path(), "P1", "call-1").unwrap();
    assert!(shown.typescript_text.contains("rev-A"));
    assert_eq!(
        load_stored_inspection(&writer, data.path(), "P2", "call-1")
            .unwrap_err()
            .code,
        CapabilityErrorCode::NotActive
    );
    std::fs::write(
        store.directory("insp-1").join("program.inspection.ts"),
        "tampered",
    )
    .unwrap();
    assert_eq!(
        load_stored_inspection(&writer, data.path(), "P1", "call-1")
            .unwrap_err()
            .code,
        CapabilityErrorCode::IntegrityError
    );
}
