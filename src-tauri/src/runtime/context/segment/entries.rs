use rusqlite::{params, Connection};

pub(crate) struct ContextEntry {
    pub(crate) sequence: i64,
    pub(crate) role: String,
    pub(crate) text: String,
}

pub(crate) fn append_conversation(
    connection: &Connection,
    segment_id: &str,
    role: &str,
    record_id: &str,
    text: &str,
) -> Result<ContextEntry, String> {
    append(connection, segment_id, role, Some(record_id), text)
}

pub(crate) fn append_tool_round(
    connection: &Connection,
    segment_id: &str,
    record_id: &str,
    summary: &str,
) -> Result<ContextEntry, String> {
    let mut text = summary.to_string();
    if text.len() > 256 {
        text.truncate(256);
        while !text.is_char_boundary(text.len()) {
            text.pop();
        }
    }
    append(connection, segment_id, "tool_round", Some(record_id), &text)
}

fn append(
    connection: &Connection,
    segment_id: &str,
    role: &str,
    record_id: Option<&str>,
    text: &str,
) -> Result<ContextEntry, String> {
    let sequence: i64 = connection
        .query_row(
            "SELECT last_entry_sequence FROM context_segments WHERE id=?1",
            [segment_id],
            |row| row.get(0),
        )
        .map_err(|error| error.to_string())?;
    let next = sequence + 1;
    let digest = crate::generated_capabilities::contracts::sha256_hex(text.as_bytes());
    let blob_id: String = if let Ok(existing) = connection.query_row(
        "SELECT id FROM blobs WHERE dedup_domain='segment' AND sha256=?1",
        [&digest],
        |row| row.get(0),
    ) {
        connection
            .execute("UPDATE blobs SET ref_count = ref_count + 1 WHERE id=?1", [&existing])
            .map_err(|error| error.to_string())?;
        existing
    } else {
        let blob_id = crate::util::new_id("blob");
        connection
            .execute(
                "INSERT INTO blobs(id, dedup_domain, sha256, codec, raw_bytes, stored_bytes, data, ref_count)
                 VALUES(?1,'segment',?2,'identity',?3,?3,?4,1)",
                params![blob_id, digest, text.len() as i64, text.as_bytes()],
            )
            .map_err(|error| error.to_string())?;
        blob_id
    };
    let inserted = connection
        .execute(
            "INSERT INTO context_entries(segment_id, sequence, role, record_id, rendered_blob_id, serializer_version, content_digest, dependency_refs_json, created_at)
             VALUES(?1,?2,?3,?4,?5,1,?6,'[]',?7)",
            params![segment_id, next, role, record_id, blob_id, digest, crate::schedule::tick::now_ms()],
        )
        .map_err(|error| error.to_string())?;
    if inserted != 1 {
        return Err("entry was not appended".into());
    }
    connection
        .execute(
            "UPDATE context_segments SET last_entry_sequence=?2 WHERE id=?1",
            params![segment_id, next],
        )
        .map_err(|error| error.to_string())?;
    Ok(ContextEntry { sequence: next, role: role.to_string(), text: text.to_string() })
}

pub(crate) fn rewrite_fails(connection: &Connection, segment_id: &str, sequence: i64) -> Result<(), String> {
    let changed = connection
        .execute(
            "UPDATE context_entries SET role='user' WHERE segment_id=?1 AND sequence=?2",
            params![segment_id, sequence],
        )
        .map_err(|error| error.to_string())?;
    if changed == 1 {
        return Err("append-only entry was rewritten".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn segment() -> (Connection, String) {
        let connection = Connection::open_in_memory().unwrap();
        crate::persistence::schema::initialize_database(&connection).unwrap();
        connection
            .execute(
                "INSERT INTO blobs(id, dedup_domain, sha256, codec, raw_bytes, stored_bytes, data, ref_count) VALUES('fixed','p','cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc','identity',1,1,X'61',1)",
                [],
            )
            .unwrap();
        let id = super::super::manifest::create(&connection, "c", "initial", None, "p", "t", "fixed", "{}", 100, 0)
            .unwrap()
            .id;
        (connection, id)
    }

    #[test]
    fn cw_42_append_is_append_only() {
        let (connection, id) = segment();
        let entry = append_conversation(&connection, &id, "user", "m1", "hello").unwrap();
        let error = connection
            .execute(
                "INSERT INTO context_entries(segment_id, sequence, role, record_id, rendered_blob_id, serializer_version, content_digest, dependency_refs_json, created_at)
                 VALUES(?1,?2,'user','m1','fixed',1,'cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc','[]',1)",
                rusqlite::params![id, entry.sequence],
            )
            .unwrap_err();
        assert!(error.to_string().contains("UNIQUE") || error.to_string().contains("constraint"));
    }

    #[test]
    fn cw_42_tool_round_summary_is_256_bytes_max() {
        let (connection, id) = segment();
        let entry = append_tool_round(&connection, &id, "r1", &"z".repeat(400)).unwrap();
        assert!(entry.text.len() <= 256);
    }
}
