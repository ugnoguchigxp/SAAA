//! Production writer connection. In-memory connections do not stand in for the journal.
use std::sync::Arc;

use saaa_larm_session::media::{
    FailureKind, MediaArtifact, MediaError, MediaKind, MediaProgress, MediaResult,
};
use saaa_media::{DbOwner, FinishOutcome, MediaStore, SqlStore};
use saaa_provider_routing::{
    AdapterKind, BindingReview, Capability, LarmReachability, Purpose, PurposeBinding,
    RegistrySnapshot, ServiceConnection, ServiceResource,
};
use serde_json::{json, Value};

use crate::persistence::SqliteWriter;

struct WriterOwner(Arc<SqliteWriter>);

impl DbOwner for WriterOwner {
    fn read<T>(
        &self,
        operation: impl FnOnce(&rusqlite::Connection) -> Result<T, String> + Send,
    ) -> Result<T, String> {
        let connection = self
            .0
            .lock()
            .map_err(|_| "Database writer unavailable".to_string())?;
        operation(&connection)
    }

    fn write<T>(
        &self,
        operation: impl FnOnce(&mut rusqlite::Connection) -> Result<T, String> + Send,
    ) -> Result<T, String> {
        self.0.write(operation)
    }
}

fn snapshot() -> RegistrySnapshot {
    RegistrySnapshot {
        connections: vec![ServiceConnection {
            connection_id: "conn:harness".into(),
            label: "LARM".into(),
            adapter_kind: AdapterKind::Larm,
            endpoint: "http://127.0.0.1:9/".into(),
            location: "local".into(),
            authentication: "none".into(),
            credential_ref: None,
            enabled: true,
        }],
        resources: vec![ServiceResource {
            resource_id: "res:harness-image".into(),
            connection_id: "conn:harness".into(),
            capability: Capability::ImageGeneration,
            model: String::new(),
            detail: None,
            request_options: None,
            enabled: true,
        }],
        bindings: vec![PurposeBinding {
            purpose: Purpose::MediaImageGenerate,
            enabled: true,
            primary_resource_id: Some("res:harness-image".into()),
            fallback_resource_ids: Vec::new(),
            cloud_allowed: false,
            timeout_ms: 1_000,
            attempt_timeout_ms: Some(1_000),
            stored_primary_resource_id: None,
            review: BindingReview::Ready,
        }],
    }
}

fn result() -> MediaResult {
    MediaResult {
        kind: MediaKind::Image,
        model: "fixture".into(),
        job_id: None,
        artifacts: vec![MediaArtifact {
            id: "artifact".into(),
            content_url: "http://127.0.0.1/fixture.png".into(),
            metadata_url: None,
            mime_type: "image/png".into(),
            metadata: json!({}),
        }],
    }
}

fn state(writer: &SqliteWriter, run: &str) -> String {
    writer
        .write(|connection| {
            connection
                .query_row(
                    "SELECT state FROM purpose_media_operations WHERE run_id=?1",
                    [run],
                    |row| row.get(0),
                )
                .map_err(|error| error.to_string())
        })
        .unwrap()
}

#[test]
fn production_writer_adopts_media_with_the_journal_and_rejects_a_second_owner() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("app.sqlite");
    let writer = Arc::new(SqliteWriter::open(&path).unwrap());
    assert!(path.with_extension("forget.json").is_file());
    match SqliteWriter::open(&path) {
        Ok(_) => panic!("a second writer must not open the same database"),
        Err(error) => assert_eq!(error.to_string(), "database-already-owned"),
    }

    let registry = snapshot();
    let route = saaa_provider_routing::resolve_route(
        &registry,
        Purpose::MediaImageGenerate,
        LarmReachability::Reachable,
    )
    .unwrap();
    writer
        .write(|connection| saaa_provider_routing::initialize_audit(connection))
        .unwrap();
    let store = SqlStore::new(
        WriterOwner(writer.clone()),
        Arc::new({
            let registry = registry.clone();
            move |_| Ok(registry.clone())
        }),
    );
    let accepted = "11111111-1111-4111-8111-111111111111";
    store
        .reserve("1", accepted, &MediaKind::Image, &route)
        .unwrap();
    assert_eq!(
        store.finish("2", accepted, &route, &Ok(result())).unwrap(),
        FinishOutcome::Accepted
    );
    assert_eq!(state(&writer, accepted), "accepted");
    let audits: i64 = writer
        .write(|connection| {
            connection
                .query_row(
                    "SELECT COUNT(*) FROM audit_events WHERE event_name='purpose-route-accepted'",
                    [],
                    |row| row.get(0),
                )
                .map_err(|error| error.to_string())
        })
        .unwrap();
    assert_eq!(audits, 1);

    let rolled = "22222222-2222-4222-8222-222222222222";
    store
        .reserve("3", rolled, &MediaKind::Image, &route)
        .unwrap();
    writer
        .write(|connection| {
            connection
                .execute("DROP TABLE audit_events", [])
                .map_err(|error| error.to_string())?;
            Ok(())
        })
        .unwrap();
    assert!(store.finish("4", rolled, &route, &Ok(result())).is_err());
    assert_eq!(state(&writer, accepted), "accepted");
    assert_eq!(state(&writer, rolled), "reserved");
}

#[test]
fn ipc_fixture_keeps_null_camel_case_enums_and_leaves_bytes_out_of_json() {
    let value = serde_json::to_value(result()).unwrap();
    assert_eq!(value["kind"], "image");
    assert_eq!(value["jobId"], Value::Null);
    assert_eq!(value["artifacts"][0]["metadataUrl"], Value::Null);
    assert_eq!(value["artifacts"][0]["mimeType"], "image/png");
    assert!(value["artifacts"][0].get("bytes").is_none());
    assert!(!value.to_string().contains("PNG"));
    let error = serde_json::to_value(MediaError {
        kind: FailureKind::OutcomeUnknown,
        code: "remote_cancel_unconfirmed".into(),
        retryable: false,
        may_have_generated: true,
        job_id: None,
    })
    .unwrap();
    assert_eq!(error["kind"], "outcomeUnknown");
    assert_eq!(error["mayHaveGenerated"], true);
    assert_eq!(error["jobId"], Value::Null);
    let progress = serde_json::to_value(MediaProgress {
        phase: "generating".into(),
        job_id: None,
        progress: None,
    })
    .unwrap();
    assert_eq!(progress["jobId"], Value::Null);
    assert_eq!(progress["progress"], Value::Null);
}
