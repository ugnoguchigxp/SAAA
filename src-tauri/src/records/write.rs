use rusqlite::{params, Connection};

use super::contract::{CaptureState, Origin, RecordKind, RecordRef};
use super::fts;
use super::outline;

pub(crate) const INLINE_LIMIT: usize = 65_536;
const STREAM_LIMIT: usize = 1_048_576;

pub(crate) struct NewRecord<'a> {
    pub(crate) kind: RecordKind,
    pub(crate) origin: Origin,
    pub(crate) principal_id: &'a str,
    pub(crate) conversation_id: &'a str,
    pub(crate) run_id: Option<&'a str>,
    pub(crate) turn_id: Option<&'a str>,
    pub(crate) parent_execution_id: Option<&'a str>,
    pub(crate) rank: Option<u32>,
    pub(crate) observed_at: i64,
    pub(crate) locator: serde_json::Value,
    pub(crate) scope_keys: &'a [String],
}

pub(crate) struct StreamingRecord {
    new: OwnedRecord,
    body: Vec<u8>,
    overflow: bool,
}

struct OwnedRecord {
    kind: RecordKind,
    origin: Origin,
    principal_id: String,
    conversation_id: String,
    run_id: Option<String>,
    turn_id: Option<String>,
    parent_execution_id: Option<String>,
    rank: Option<u32>,
    observed_at: i64,
    locator: serde_json::Value,
    scope_keys: Vec<String>,
    id: String,
}

impl<'a> NewRecord<'a> {
    fn own(&self) -> OwnedRecord {
        OwnedRecord {
            kind: self.kind,
            origin: self.origin,
            principal_id: self.principal_id.to_string(),
            conversation_id: self.conversation_id.to_string(),
            run_id: self.run_id.map(str::to_string),
            turn_id: self.turn_id.map(str::to_string),
            parent_execution_id: self.parent_execution_id.map(str::to_string),
            rank: self.rank,
            observed_at: self.observed_at,
            locator: self.locator.clone(),
            scope_keys: self.scope_keys.to_vec(),
            id: crate::util::new_id("record"),
        }
    }
}

pub(crate) fn commit(
    connection: &Connection,
    new: NewRecord<'_>,
    received_body: &[u8],
    readable_text: Option<&str>,
) -> Result<RecordRef, String> {
    finish_owned(
        connection,
        new.own(),
        received_body,
        CaptureState::Complete,
        None,
        readable_text,
    )
}

pub(crate) fn begin(
    connection: &Connection,
    new: NewRecord<'_>,
) -> Result<StreamingRecord, String> {
    let owned = new.own();
    insert_record(connection, &owned, CaptureState::Streaming, None)?;
    Ok(StreamingRecord {
        new: owned,
        body: Vec::new(),
        overflow: false,
    })
}

pub(crate) fn append_chunk(
    connection: &Connection,
    rec: &mut StreamingRecord,
    bytes: &[u8],
) -> Result<(), String> {
    let _ = connection;
    if rec.overflow {
        return Ok(());
    }
    if rec.body.len() + bytes.len() > STREAM_LIMIT {
        let room = STREAM_LIMIT.saturating_sub(rec.body.len());
        rec.body.extend_from_slice(&bytes[..room]);
        rec.overflow = true;
        return Ok(());
    }
    rec.body.extend_from_slice(bytes);
    Ok(())
}

pub(crate) fn finish(
    connection: &Connection,
    rec: StreamingRecord,
    state: CaptureState,
    readable_text: Option<&str>,
) -> Result<RecordRef, String> {
    let (state, reason) = if rec.overflow {
        (CaptureState::Partial, Some("stream_truncated"))
    } else {
        (state, None)
    };
    connection
        .execute("DELETE FROM records WHERE id=?1", [&rec.new.id])
        .map_err(|error| error.to_string())?;
    finish_owned(connection, rec.new, &rec.body, state, reason, readable_text)
}

pub(crate) fn abort(connection: &Connection, rec: StreamingRecord) -> Result<(), String> {
    connection
        .execute(
            "UPDATE records SET capture_state='failed', capture_reason='aborted', recorded_at=?2 WHERE id=?1",
            params![rec.new.id, crate::schedule::tick::now_ms()],
        )
        .map_err(|error| error.to_string())?;
    Ok(())
}

fn finish_owned(
    connection: &Connection,
    new: OwnedRecord,
    received_body: &[u8],
    state: CaptureState,
    reason: Option<&str>,
    readable_text: Option<&str>,
) -> Result<RecordRef, String> {
    insert_record(connection, &new, state, reason)?;
    let (blob_id, sha) = store_blob(connection, &new.principal_id, received_body)?;
    bind_representation(
        connection,
        &new.id,
        "received_body",
        &blob_id,
        &sha,
        received_body.len(),
        "identity",
    )?;
    let mut outline_value = None;
    if let Some(text) = readable_text {
        let (text_blob, text_sha) = store_blob(connection, &new.principal_id, text.as_bytes())?;
        bind_representation(
            connection,
            &new.id,
            "readable_text",
            &text_blob,
            &text_sha,
            text.len(),
            "same-as-received",
        )?;
        fts::index_text(connection, &new.id, "readable_text", text)?;
        outline_value = Some(outline::build(outline::OutlineKind::Plain, text));
    }
    for key in &new.scope_keys {
        connection
            .execute(
                "INSERT INTO record_scopes(record_id, scope_key, relation, policy_revision) VALUES(?1,?2,'visible',0)",
                params![new.id, key],
            )
            .map_err(|error| error.to_string())?;
    }
    Ok(RecordRef {
        id: new.id,
        sha256: sha,
        bytes: received_body.len() as u64,
        outline: outline_value,
    })
}

fn insert_record(
    connection: &Connection,
    new: &OwnedRecord,
    state: CaptureState,
    reason: Option<&str>,
) -> Result<(), String> {
    let now = crate::schedule::tick::now_ms();
    connection
        .execute(
            "INSERT INTO records(
               id, kind, origin, principal_id, conversation_id, run_id, turn_id,
               parent_execution_id, rank, observed_at, recorded_at, locator_json,
               capture_state, capture_reason, version, existing_source_locator, forget_epoch
             ) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,1,NULL,NULL)",
            params![
                new.id,
                new.kind.as_str(),
                new.origin.as_str(),
                new.principal_id,
                new.conversation_id,
                new.run_id,
                new.turn_id,
                new.parent_execution_id,
                new.rank.map(|rank| rank as i64),
                new.observed_at,
                now,
                new.locator.to_string(),
                state.as_str(),
                reason,
            ],
        )
        .map_err(|error| error.to_string())?;
    Ok(())
}

fn store_blob(
    connection: &Connection,
    domain: &str,
    bytes: &[u8],
) -> Result<(String, String), String> {
    let sha = crate::generated_capabilities::contracts::sha256_hex(bytes);
    if let Some(id) = connection
        .query_row(
            "SELECT id FROM blobs WHERE dedup_domain=?1 AND sha256=?2",
            params![domain, sha],
            |row| row.get::<_, String>(0),
        )
        .ok()
    {
        connection
            .execute(
                "UPDATE blobs SET ref_count = ref_count + 1 WHERE id=?1",
                [&id],
            )
            .map_err(|error| error.to_string())?;
        return Ok((id, sha));
    }
    let id = crate::util::new_id("blob");
    if bytes.len() <= INLINE_LIMIT {
        connection
            .execute(
                "INSERT INTO blobs(id, dedup_domain, sha256, codec, raw_bytes, stored_bytes, data, ref_count)
                 VALUES(?1,?2,?3,'identity',?4,?4,?5,1)",
                params![id, domain, sha, bytes.len() as i64, bytes],
            )
            .map_err(|error| error.to_string())?;
    } else {
        connection
            .execute(
                "INSERT INTO blobs(id, dedup_domain, sha256, codec, raw_bytes, stored_bytes, data, ref_count)
                 VALUES(?1,?2,?3,'identity',?4,?4,NULL,1)",
                params![id, domain, sha, bytes.len() as i64],
            )
            .map_err(|error| error.to_string())?;
        let mut offset = 0usize;
        let mut sequence = 0i64;
        while offset < bytes.len() {
            let end = (offset + INLINE_LIMIT).min(bytes.len());
            let chunk = &bytes[offset..end];
            let chunk_sha = crate::generated_capabilities::contracts::sha256_hex(chunk);
            connection
                .execute(
                    "INSERT INTO blob_chunks(blob_id, sequence, raw_offset, raw_bytes, codec, data, sha256)
                     VALUES(?1,?2,?3,?4,'identity',?5,?6)",
                    params![id, sequence, offset as i64, chunk.len() as i64, chunk, chunk_sha],
                )
                .map_err(|error| error.to_string())?;
            offset = end;
            sequence += 1;
        }
    }
    Ok((id, sha))
}

fn bind_representation(
    connection: &Connection,
    record_id: &str,
    name: &str,
    blob_id: &str,
    sha: &str,
    len: usize,
    parser: &str,
) -> Result<(), String> {
    connection
        .execute(
            "INSERT INTO record_representations(record_id, name, blob_id, sha256, byte_length, parser_version)
             VALUES(?1,?2,?3,?4,?5,?6)",
            params![record_id, name, blob_id, sha, len as i64, parser],
        )
        .map_err(|error| error.to_string())?;
    Ok(())
}

pub(crate) fn load_blob(connection: &Connection, blob_id: &str) -> Result<Vec<u8>, String> {
    let inline: Option<Vec<u8>> = connection
        .query_row("SELECT data FROM blobs WHERE id=?1", [blob_id], |row| {
            row.get(0)
        })
        .map_err(|error| error.to_string())?;
    if let Some(data) = inline {
        return Ok(data);
    }
    let mut statement = connection
        .prepare("SELECT data FROM blob_chunks WHERE blob_id=?1 ORDER BY sequence")
        .map_err(|error| error.to_string())?;
    let chunks = statement
        .query_map([blob_id], |row| row.get::<_, Vec<u8>>(0))
        .map_err(|error| error.to_string())?;
    let mut body = Vec::new();
    for chunk in chunks {
        body.extend(chunk.map_err(|error| error.to_string())?);
    }
    Ok(body)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn db() -> Connection {
        let connection = Connection::open_in_memory().unwrap();
        crate::persistence::schema::initialize_database(&connection).unwrap();
        connection
    }

    fn new<'a>(scopes: &'a [String]) -> NewRecord<'a> {
        NewRecord {
            kind: RecordKind::WebFetch,
            origin: Origin::ExternalObservation,
            principal_id: "principal",
            conversation_id: "conversation",
            run_id: Some("run"),
            turn_id: None,
            parent_execution_id: None,
            rank: None,
            observed_at: 1,
            locator: serde_json::json!({"tool": "fetch_content"}),
            scope_keys: scopes,
        }
    }

    #[test]
    fn cw_22_commit_small_body_inlines_blob() {
        let connection = db();
        let stored = commit(&connection, new(&[]), b"hello", Some("hello")).unwrap();
        let inline: Option<Vec<u8>> = connection
            .query_row(
                "SELECT data FROM blobs WHERE sha256=?1",
                [&stored.sha256],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(inline.unwrap(), b"hello");
    }

    #[test]
    fn cw_22_commit_large_body_uses_chunks() {
        let connection = db();
        let body = vec![7u8; 200 * 1024];
        let stored = commit(&connection, new(&[]), &body, None).unwrap();
        let (data_null, chunks, first, last): (bool, i64, i64, i64) = connection
            .query_row(
                "SELECT data IS NULL,
                        (SELECT count(*) FROM blob_chunks c JOIN blobs b ON b.id=c.blob_id WHERE b.sha256=?1),
                        (SELECT min(raw_offset) FROM blob_chunks c JOIN blobs b ON b.id=c.blob_id WHERE b.sha256=?1),
                        (SELECT max(raw_offset) FROM blob_chunks c JOIN blobs b ON b.id=c.blob_id WHERE b.sha256=?1)
                 FROM blobs WHERE sha256=?1",
                [&stored.sha256],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .unwrap();
        assert!(data_null);
        assert_eq!(chunks, 4);
        assert_eq!(first, 0);
        assert_eq!(last, 65_536 * 3);
    }

    #[test]
    fn cw_22_same_body_shares_blob_and_increments_ref_count() {
        let connection = db();
        commit(&connection, new(&[]), b"same", None).unwrap();
        commit(&connection, new(&[]), b"same", None).unwrap();
        let (blobs, refs): (i64, i64) = connection
            .query_row("SELECT count(*), max(ref_count) FROM blobs", [], |row| {
                Ok((row.get(0)?, row.get(1)?))
            })
            .unwrap();
        assert_eq!(blobs, 1);
        assert_eq!(refs, 2);
    }

    #[test]
    fn cw_23_finish_partial_records_reason() {
        let connection = db();
        let mut rec = begin(&connection, new(&[])).unwrap();
        append_chunk(&connection, &mut rec, &vec![1u8; STREAM_LIMIT + 8]).unwrap();
        let stored = finish(&connection, rec, CaptureState::Complete, None).unwrap();
        let (state, reason): (String, String) = connection
            .query_row(
                "SELECT capture_state, capture_reason FROM records WHERE id=?1",
                [&stored.id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(state, "partial");
        assert_eq!(reason, "stream_truncated");
        assert_eq!(stored.bytes, STREAM_LIMIT as u64);
    }

    #[test]
    fn cw_23_abort_marks_failed_without_blob() {
        let connection = db();
        let rec = begin(&connection, new(&[])).unwrap();
        let id = rec.new.id.clone();
        abort(&connection, rec).unwrap();
        let state: String = connection
            .query_row(
                "SELECT capture_state FROM records WHERE id=?1",
                [&id],
                |row| row.get(0),
            )
            .unwrap();
        let blobs: i64 = connection
            .query_row("SELECT count(*) FROM blobs", [], |row| row.get(0))
            .unwrap();
        assert_eq!(state, "failed");
        assert_eq!(blobs, 0);
    }
}
