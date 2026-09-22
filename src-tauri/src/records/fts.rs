use rusqlite::{params, Connection};

const CHUNK_BYTES: usize = 8_192;
const INDEX_LIMIT: usize = 1_048_576;

pub(crate) fn index_text(
    connection: &Connection,
    record_id: &str,
    representation: &str,
    text: &str,
) -> Result<(), String> {
    let bytes = text.as_bytes();
    let limit = bytes.len().min(INDEX_LIMIT);
    let mut start = 0usize;
    let mut version = 1i64;
    while start < limit {
        let mut end = (start + CHUNK_BYTES).min(limit);
        while end > start && !text.is_char_boundary(end) {
            end -= 1;
        }
        if end == start {
            return Err("index chunk is not valid utf-8".into());
        }
        let slice = &text[start..end];
        connection
            .execute(
                "INSERT INTO record_text_chunks(record_id, representation, start_byte, end_byte, text, index_version)
                 VALUES(?1,?2,?3,?4,?5,?6)",
                params![record_id, representation, start as i64, end as i64, slice, version],
            )
            .map_err(|error| error.to_string())?;
        let rowid = connection.last_insert_rowid();
        connection
            .execute(
                "INSERT INTO record_fts(rowid, text) VALUES(?1,?2)",
                params![rowid, slice],
            )
            .map_err(|error| error.to_string())?;
        if end == limit {
            break;
        }
        let mut next = end;
        let mut chars = 0;
        while next > start && chars < 2 {
            next -= 1;
            if text.is_char_boundary(next) {
                chars += 1;
            }
        }
        start = next;
        version += 1;
        if start >= end {
            start = end;
        }
    }
    Ok(())
}

pub(crate) fn delete_text(connection: &Connection, record_id: &str) -> Result<(), String> {
    let mut statement = connection
        .prepare("SELECT id, text FROM record_text_chunks WHERE record_id=?1")
        .map_err(|error| error.to_string())?;
    let rows = statement
        .query_map(params![record_id], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(|error| error.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string())?;
    drop(statement);
    for (rowid, text) in rows {
        connection
            .execute(
                "INSERT INTO record_fts(record_fts, rowid, text) VALUES('delete', ?1, ?2)",
                params![rowid, text],
            )
            .map_err(|error| error.to_string())?;
    }
    connection
        .execute(
            "DELETE FROM record_text_chunks WHERE record_id=?1",
            params![record_id],
        )
        .map_err(|error| error.to_string())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::records::write::{commit, NewRecord};
    use crate::records::{CaptureState, Origin, RecordKind};

    fn db() -> Connection {
        let connection = Connection::open_in_memory().unwrap();
        crate::persistence::schema::initialize_database(&connection).unwrap();
        connection
    }

    fn sample<'a>(scopes: &'a [String]) -> NewRecord<'a> {
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
            locator: serde_json::json!({"url": "https://example.test"}),
            scope_keys: scopes,
        }
    }

    #[test]
    fn cw_24_index_chunks_align_utf8() {
        let connection = db();
        let text = "あ".repeat(100_000);
        let stored = commit(&connection, sample(&[]), text.as_bytes(), Some(&text)).unwrap();
        let mut statement = connection
            .prepare("SELECT start_byte, end_byte, text FROM record_text_chunks WHERE record_id=?1")
            .unwrap();
        let rows = statement
            .query_map([&stored.id], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })
            .unwrap();
        for row in rows {
            let (start, end, chunk) = row.unwrap();
            assert!(text.is_char_boundary(start as usize));
            assert!(text.is_char_boundary(end as usize));
            assert_eq!(&text[start as usize..end as usize], chunk);
        }
    }

    #[test]
    fn cw_24_search_hits_after_first_64kib() {
        let connection = db();
        let mut text = "あ".repeat(70_000);
        text.push_str("検索語彙xyz");
        commit(&connection, sample(&[]), text.as_bytes(), Some(&text)).unwrap();
        let hits: i64 = connection
            .query_row(
                "SELECT count(*) FROM record_fts WHERE record_fts MATCH '検索語彙'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(hits >= 1);
    }

    #[test]
    fn cw_24_delete_removes_fts_rows() {
        let connection = db();
        let stored = commit(
            &connection,
            sample(&[]),
            b"alpha beta gamma",
            Some("alpha beta gamma"),
        )
        .unwrap();
        delete_text(&connection, &stored.id).unwrap();
        let chunks: i64 = connection
            .query_row(
                "SELECT count(*) FROM record_text_chunks WHERE record_id=?1",
                [&stored.id],
                |row| row.get(0),
            )
            .unwrap();
        let hits: i64 = connection
            .query_row(
                "SELECT count(*) FROM record_fts WHERE record_fts MATCH 'alpha'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(chunks, 0);
        assert_eq!(hits, 0);
        let _ = CaptureState::Complete;
    }
}
