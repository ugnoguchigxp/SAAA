use rusqlite::{params, params_from_iter, Connection};

use super::auth::Authorization;
use super::contract::{CaptureState, RecordKind};
use super::write;

pub(crate) struct ReadRange {
    pub(crate) start: u64,
    pub(crate) max_bytes: u32,
}

pub(crate) struct ReadResult {
    pub(crate) record_id: String,
    pub(crate) representation: String,
    pub(crate) sha256: String,
    pub(crate) actual_start: u64,
    pub(crate) actual_end: u64,
    pub(crate) text: String,
    pub(crate) capture_state: CaptureState,
    pub(crate) source_refs: Vec<String>,
    pub(crate) truncated: bool,
}

pub(crate) struct SearchHit {
    pub(crate) start_byte: u64,
    pub(crate) end_byte: u64,
    pub(crate) snippet: String,
}

pub(crate) struct ActivityQuery {
    pub(crate) kinds: Vec<RecordKind>,
    pub(crate) run_id: Option<String>,
    pub(crate) parent_id: Option<String>,
    pub(crate) rank: Option<u32>,
    pub(crate) before_record_id: Option<String>,
    pub(crate) since_ms: Option<i64>,
    pub(crate) until_ms: Option<i64>,
    pub(crate) query: Option<String>,
    pub(crate) limit: u8,
    pub(crate) cursor: Option<String>,
}

pub(crate) struct ActivityPage {
    pub(crate) items: Vec<String>,
    pub(crate) next_cursor: Option<String>,
    pub(crate) coverage: String,
}

pub(crate) fn read_range(
    connection: &Connection,
    auth: &Authorization,
    record_id: &str,
    representation: &str,
    range: ReadRange,
) -> Result<Option<ReadResult>, String> {
    if !visible(connection, auth, record_id)? {
        return Ok(None);
    }
    let max_bytes = range.max_bytes.min(8_192) as usize;
    let (blob_id, sha, capture): (String, String, String) = match connection.query_row(
        "SELECT rr.blob_id, rr.sha256, r.capture_state
         FROM record_representations rr
         JOIN records r ON r.id = rr.record_id
         WHERE rr.record_id=?1 AND rr.name=?2",
        params![record_id, representation],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
    ) {
        Ok(row) => row,
        Err(rusqlite::Error::QueryReturnedNoRows) => return Ok(None),
        Err(error) => return Err(error.to_string()),
    };
    let body = write::load_blob(connection, &blob_id)?;
    let text = String::from_utf8_lossy(&body);
    let mut start = (range.start as usize).min(text.len());
    while start > 0 && !text.is_char_boundary(start) {
        start -= 1;
    }
    let mut end = (start + max_bytes).min(text.len());
    while end > start && !text.is_char_boundary(end) {
        end -= 1;
    }
    let capture_state = match capture.as_str() {
        "streaming" => CaptureState::Streaming,
        "partial" => CaptureState::Partial,
        "failed" => CaptureState::Failed,
        _ => CaptureState::Complete,
    };
    Ok(Some(ReadResult {
        record_id: record_id.to_string(),
        representation: representation.to_string(),
        sha256: sha,
        actual_start: start as u64,
        actual_end: end as u64,
        text: text[start..end].to_string(),
        capture_state,
        source_refs: Vec::new(),
        truncated: end < text.len(),
    }))
}

pub(crate) fn search_in_record(
    connection: &Connection,
    auth: &Authorization,
    record_id: &str,
    query: &str,
    limit: u8,
) -> Result<Option<Vec<SearchHit>>, String> {
    if !visible(connection, auth, record_id)? {
        return Ok(None);
    }
    let mut hits = Vec::new();
    if query.chars().count() < 3 {
        let pattern = format!("%{query}%");
        let mut statement = connection
            .prepare(
                "SELECT start_byte, end_byte, text FROM record_text_chunks
                 WHERE record_id=?1 AND text LIKE ?2 ORDER BY start_byte LIMIT ?3",
            )
            .map_err(|error| error.to_string())?;
        let rows = statement
            .query_map(params![record_id, pattern, limit], |row| {
                Ok(SearchHit {
                    start_byte: row.get::<_, i64>(0)? as u64,
                    end_byte: row.get::<_, i64>(1)? as u64,
                    snippet: row.get(2)?,
                })
            })
            .map_err(|error| error.to_string())?;
        for row in rows {
            hits.push(row.map_err(|error| error.to_string())?);
        }
        return Ok(Some(hits));
    }
    let mut statement = connection
        .prepare(
            "SELECT c.start_byte, c.end_byte, c.text
             FROM record_text_chunks c
             JOIN record_fts f ON f.rowid = c.id
             WHERE c.record_id=?1 AND record_fts MATCH ?2
             ORDER BY c.start_byte LIMIT ?3",
        )
        .map_err(|error| error.to_string())?;
    let rows = statement
        .query_map(params![record_id, query, limit], |row| {
            Ok(SearchHit {
                start_byte: row.get::<_, i64>(0)? as u64,
                end_byte: row.get::<_, i64>(1)? as u64,
                snippet: row.get(2)?,
            })
        })
        .map_err(|error| error.to_string())?;
    for row in rows {
        hits.push(row.map_err(|error| error.to_string())?);
    }
    Ok(Some(hits))
}

pub(crate) fn list_activity(
    connection: &Connection,
    auth: &Authorization,
    query: ActivityQuery,
) -> Result<ActivityPage, String> {
    let (filter, filter_params) = auth.sql_filter("r");
    let mut sql = format!("SELECT r.id, r.recorded_at FROM records r WHERE {filter}");
    let mut values = filter_params;
    if !query.kinds.is_empty() {
        let marks = vec!["?"; query.kinds.len()].join(",");
        sql.push_str(&format!(" AND r.kind IN ({marks})"));
        for kind in &query.kinds {
            values.push(rusqlite::types::Value::Text(kind.as_str().to_string()));
        }
    }
    if let Some(run_id) = &query.run_id {
        sql.push_str(" AND r.run_id=?");
        values.push(rusqlite::types::Value::Text(run_id.clone()));
    }
    if let Some(parent) = &query.parent_id {
        sql.push_str(" AND r.parent_execution_id=?");
        values.push(rusqlite::types::Value::Text(parent.clone()));
    }
    if let Some(rank) = query.rank {
        sql.push_str(" AND r.rank=?");
        values.push(rusqlite::types::Value::Integer(rank as i64));
    }
    if let Some(cursor) = &query.cursor {
        if let Some((recorded_at, id)) = decode_cursor(cursor) {
            sql.push_str(" AND (r.recorded_at < ? OR (r.recorded_at = ? AND r.id < ?))");
            values.push(rusqlite::types::Value::Integer(recorded_at));
            values.push(rusqlite::types::Value::Integer(recorded_at));
            values.push(rusqlite::types::Value::Text(id));
        }
    }
    let limit = query.limit.clamp(1, 20) as i64;
    sql.push_str(" ORDER BY r.recorded_at DESC, r.id DESC LIMIT ?");
    values.push(rusqlite::types::Value::Integer(limit + 1));
    let mut statement = connection
        .prepare(&sql)
        .map_err(|error| error.to_string())?;
    let rows = statement
        .query_map(params_from_iter(values.iter()), |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
        })
        .map_err(|error| error.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string())?;
    let mut items = rows;
    let next_cursor = if items.len() as i64 > limit {
        items.pop();
        items
            .last()
            .map(|(id, at)| encode_cursor(*at, id))
    } else {
        None
    };
    Ok(ActivityPage {
        items: items.into_iter().map(|(id, _)| id).collect(),
        next_cursor,
        coverage: "complete".into(),
    })
}

fn visible(connection: &Connection, auth: &Authorization, record_id: &str) -> Result<bool, String> {
    let (filter, mut params_list) = auth.sql_filter("r");
    let sql = format!("SELECT 1 FROM records r WHERE r.id=? AND {filter} LIMIT 1");
    let mut values = vec![rusqlite::types::Value::Text(record_id.to_string())];
    values.append(&mut params_list);
    match connection.query_row(
        &sql,
        params_from_iter(values.iter()),
        |_| Ok(true),
    ) {
        Ok(found) => Ok(found),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(false),
        Err(error) => Err(error.to_string()),
    }
}

fn encode_cursor(recorded_at: i64, id: &str) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(format!("{recorded_at}:{id}"))
}

fn decode_cursor(cursor: &str) -> Option<(i64, String)> {
    use base64::Engine;
    let bytes = base64::engine::general_purpose::STANDARD.decode(cursor).ok()?;
    let text = String::from_utf8(bytes).ok()?;
    let (at, id) = text.split_once(':')?;
    Some((at.parse().ok()?, id.to_string()))
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

    fn put(connection: &Connection, principal: &str, conversation: &str, scopes: &[String], body: &str) -> String {
        commit(
            connection,
            NewRecord {
                kind: RecordKind::WebFetch,
                origin: Origin::ExternalObservation,
                principal_id: principal,
                conversation_id: conversation,
                run_id: None,
                turn_id: None,
                parent_execution_id: None,
                rank: None,
                observed_at: 1,
                locator: serde_json::json!({}),
                scope_keys: scopes,
            },
            body.as_bytes(),
            Some(body),
        )
        .unwrap()
        .id
    }

    #[test]
    fn cw_25_read_range_adjusts_utf8_boundary() {
        let connection = db();
        let id = put(&connection, "p", "c", &[], "あいう");
        let auth = Authorization {
            principal_id: "p".into(),
            conversation_id: "c".into(),
            allowed_scope_keys: vec![],
        };
        let result = read_range(
            &connection,
            &auth,
            &id,
            "readable_text",
            ReadRange { start: 1, max_bytes: 3 },
        )
        .unwrap()
        .unwrap();
        assert!(result.text.is_char_boundary(0));
        assert_eq!(result.actual_start, 0);
    }

    #[test]
    fn cw_25_unauthorized_and_missing_are_both_none() {
        let connection = db();
        let id = put(&connection, "p", "c", &[], "body");
        let other = Authorization {
            principal_id: "other".into(),
            conversation_id: "c".into(),
            allowed_scope_keys: vec![],
        };
        let owner = Authorization {
            principal_id: "p".into(),
            conversation_id: "c".into(),
            allowed_scope_keys: vec![],
        };
        assert!(read_range(&connection, &other, &id, "readable_text", ReadRange { start: 0, max_bytes: 10 }).unwrap().is_none());
        assert!(read_range(&connection, &owner, "missing", "readable_text", ReadRange { start: 0, max_bytes: 10 }).unwrap().is_none());
    }

    #[test]
    fn cw_25_scope_filter_applies_before_limit() {
        let connection = db();
        let other = vec!["project:other".into()];
        let mine = vec!["project:mine".into()];
        for index in 0..30 {
            put(&connection, "p", "c", &other, &format!("other-{index}"));
        }
        for index in 0..5 {
            put(&connection, "p", "c", &mine, &format!("mine-{index}"));
        }
        let auth = Authorization {
            principal_id: "p".into(),
            conversation_id: "other-conversation".into(),
            allowed_scope_keys: mine,
        };
        let page = list_activity(
            &connection,
            &auth,
            ActivityQuery {
                kinds: vec![],
                run_id: None,
                parent_id: None,
                rank: None,
                before_record_id: None,
                since_ms: None,
                until_ms: None,
                query: None,
                limit: 10,
                cursor: None,
            },
        )
        .unwrap();
        assert_eq!(page.items.len(), 5);
    }

    #[test]
    fn cw_25_list_activity_rank_filter() {
        let connection = db();
        let scopes = Vec::new();
        commit(
            &connection,
            NewRecord {
                kind: RecordKind::WebSearchResult,
                origin: Origin::ExternalObservation,
                principal_id: "p",
                conversation_id: "c",
                run_id: None,
                turn_id: None,
                parent_execution_id: Some("search"),
                rank: Some(2),
                observed_at: 1,
                locator: serde_json::json!({}),
                scope_keys: &scopes,
            },
            b"second",
            Some("second"),
        )
        .unwrap();
        let page = list_activity(
            &connection,
            &Authorization {
                principal_id: "p".into(),
                conversation_id: "c".into(),
                allowed_scope_keys: vec![],
            },
            ActivityQuery {
                kinds: vec![RecordKind::WebSearchResult],
                run_id: None,
                parent_id: Some("search".into()),
                rank: Some(2),
                before_record_id: None,
                since_ms: None,
                until_ms: None,
                query: None,
                limit: 10,
                cursor: None,
            },
        )
        .unwrap();
        assert_eq!(page.items.len(), 1);
    }

    #[test]
    fn cw_25_cursor_is_stable_across_inserts() {
        let connection = db();
        let mut ids = Vec::new();
        for index in 0..3 {
            ids.push(put(&connection, "p", "c", &[], &format!("row-{index}")));
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        let auth = Authorization {
            principal_id: "p".into(),
            conversation_id: "c".into(),
            allowed_scope_keys: vec![],
        };
        let query = |cursor: Option<String>| ActivityQuery {
            kinds: vec![],
            run_id: None,
            parent_id: None,
            rank: None,
            before_record_id: None,
            since_ms: None,
            until_ms: None,
            query: None,
            limit: 1,
            cursor,
        };
        let first = list_activity(&connection, &auth, query(None)).unwrap();
        let cursor = first.next_cursor.clone().unwrap();
        put(&connection, "p", "c", &[], "newer");
        let second = list_activity(&connection, &auth, query(Some(cursor))).unwrap();
        assert_ne!(second.items[0], first.items[0]);
        assert!(!second.items.contains(&"missing".to_string()));
    }
}
