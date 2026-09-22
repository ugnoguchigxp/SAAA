use rusqlite::{params, Connection};

use super::fts;

pub(crate) fn forget_record(connection: &Connection, record_id: &str, reason: &str) -> Result<(), String> {
    let epoch = crate::schedule::tick::now_ms();
    connection
        .execute(
            "INSERT OR IGNORE INTO record_tombstones(record_id, forgotten_at, forget_epoch, reason_code) VALUES(?1,?2,?2,?3)",
            params![record_id, epoch, reason],
        )
        .map_err(|error| error.to_string())?;
    connection
        .execute(
            "UPDATE records SET forget_epoch=?2 WHERE id=?1",
            params![record_id, epoch],
        )
        .map_err(|error| error.to_string())?;
    connection
        .execute("DELETE FROM record_scopes WHERE record_id=?1", [record_id])
        .map_err(|error| error.to_string())?;
    fts::delete_text(connection, record_id)?;
    let blob_ids = blob_ids(connection, record_id)?;
    connection
        .execute("DELETE FROM record_representations WHERE record_id=?1", [record_id])
        .map_err(|error| error.to_string())?;
    for blob_id in blob_ids {
        connection
            .execute("UPDATE blobs SET ref_count = ref_count - 1 WHERE id=?1", [&blob_id])
            .map_err(|error| error.to_string())?;
        let refs: i64 = connection
            .query_row("SELECT ref_count FROM blobs WHERE id=?1", [&blob_id], |row| row.get(0))
            .unwrap_or(1);
        if refs <= 0 {
            connection
                .execute("DELETE FROM blob_chunks WHERE blob_id=?1", [&blob_id])
                .map_err(|error| error.to_string())?;
            connection
                .execute("DELETE FROM blobs WHERE id=?1", [&blob_id])
                .map_err(|error| error.to_string())?;
        }
    }
    invalidate_segments(connection, record_id)?;
    Ok(())
}

pub(crate) fn forget_by_conversation_messages(
    connection: &Connection,
    message_ids: &[String],
    epoch: i64,
) -> Result<(), String> {
    for id in message_ids {
        let mut statement = connection
            .prepare("SELECT id FROM records WHERE existing_source_locator=?1 AND forget_epoch IS NULL")
            .map_err(|error| error.to_string())?;
        let ids = statement
            .query_map(params![id], |row| row.get::<_, String>(0))
            .map_err(|error| error.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| error.to_string())?;
        drop(statement);
        for record_id in ids {
            forget_record(connection, &record_id, "conversation_message")?;
        }
        let _ = epoch;
    }
    Ok(())
}

fn blob_ids(connection: &Connection, record_id: &str) -> Result<Vec<String>, String> {
    let mut statement = connection
        .prepare("SELECT blob_id FROM record_representations WHERE record_id=?1")
        .map_err(|error| error.to_string())?;
    statement
        .query_map([record_id], |row| row.get(0))
        .map_err(|error| error.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string())
}

fn invalidate_segments(connection: &Connection, record_id: &str) -> Result<(), String> {
    connection
        .execute(
            "UPDATE context_segments SET status='invalidated'
             WHERE id IN (
               SELECT segment_id FROM context_entries WHERE record_id=?1
             )",
            [record_id],
        )
        .ok();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::records::write::{commit, NewRecord};
    use crate::records::{Origin, RecordKind};

    fn db() -> Connection {
        let connection = Connection::open_in_memory().unwrap();
        crate::persistence::schema::initialize_database(&connection).unwrap();
        connection
    }

    fn put(connection: &Connection, body: &[u8]) -> String {
        commit(
            connection,
            NewRecord {
                kind: RecordKind::WebFetch,
                origin: Origin::ExternalObservation,
                principal_id: "p",
                conversation_id: "c",
                run_id: None,
                turn_id: None,
                parent_execution_id: None,
                rank: None,
                observed_at: 1,
                locator: serde_json::json!({}),
                scope_keys: &[],
            },
            body,
            Some(std::str::from_utf8(body).unwrap_or("x")),
        )
        .unwrap()
        .id
    }

    #[test]
    fn cw_50_forget_removes_fts_and_blob_when_last_ref() {
        let connection = db();
        let id = put(&connection, b"unique-body");
        forget_record(&connection, &id, "user").unwrap();
        let blobs: i64 = connection.query_row("SELECT count(*) FROM blobs", [], |row| row.get(0)).unwrap();
        let hits: i64 = connection
            .query_row("SELECT count(*) FROM record_fts WHERE record_fts MATCH 'unique'", [], |row| row.get(0))
            .unwrap();
        assert_eq!(blobs, 0);
        assert_eq!(hits, 0);
    }

    #[test]
    fn cw_50_forget_keeps_shared_blob() {
        let connection = db();
        let first = put(&connection, b"shared");
        let _second = put(&connection, b"shared");
        forget_record(&connection, &first, "user").unwrap();
        let refs: i64 = connection.query_row("SELECT max(ref_count) FROM blobs", [], |row| row.get(0)).unwrap();
        assert!(refs >= 1);
    }

    #[test]
    fn cw_50_forget_invalidates_dependent_segment() {
        let connection = db();
        let id = put(&connection, b"seg");
        connection
            .execute(
                "INSERT INTO blobs(id, dedup_domain, sha256, codec, raw_bytes, stored_bytes, data, ref_count)
                 VALUES('blob-fixed','p','aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa','identity',1,1,X'61',1)",
                [],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO context_segments(id, conversation_id, start_reason, scope_snapshot_json, policy_version, bootstrap_tool_schema_digest, renderer_version, adapter_contract_version, fixed_render_blob_id, last_entry_sequence, input_budget, forget_epoch, created_at, status)
                 VALUES('seg','c','initial','{}','p','b',1,1,'blob-fixed',1,1000,0,1,'active')",
                [],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO context_entries(segment_id, sequence, role, record_id, rendered_blob_id, serializer_version, content_digest, dependency_refs_json, created_at)
                 VALUES('seg',1,'user',?1,'blob-fixed',1,'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa','[]',1)",
                [&id],
            )
            .unwrap();
        forget_record(&connection, &id, "user").unwrap();
        let status: String = connection
            .query_row("SELECT status FROM context_segments WHERE id='seg'", [], |row| row.get(0))
            .unwrap();
        assert_eq!(status, "invalidated");
    }

    #[test]
    fn cw_50_forgotten_record_is_unavailable() {
        let connection = db();
        let id = put(&connection, b"gone");
        forget_record(&connection, &id, "user").unwrap();
        let visible = crate::records::read::read_range(
            &connection,
            &crate::records::auth::Authorization {
                principal_id: "p".into(),
                conversation_id: "c".into(),
                allowed_scope_keys: vec![],
            },
            &id,
            "readable_text",
            crate::records::read::ReadRange { start: 0, max_bytes: 10 },
        )
        .unwrap();
        assert!(visible.is_none());
    }
}
