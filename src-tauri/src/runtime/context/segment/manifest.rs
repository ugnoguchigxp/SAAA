use rusqlite::{params, Connection};

pub(crate) struct SegmentManifest {
    pub(crate) id: String,
    pub(crate) conversation_id: String,
    pub(crate) previous_segment_id: Option<String>,
    pub(crate) start_reason: String,
    pub(crate) policy_version: String,
    pub(crate) bootstrap_tool_schema_digest: String,
    pub(crate) fixed_render_blob_id: String,
    pub(crate) scope_snapshot_json: String,
    pub(crate) forget_epoch: i64,
    pub(crate) input_budget: i64,
    pub(crate) last_entry_sequence: i64,
}

pub(crate) fn load_active(
    connection: &Connection,
    conversation_id: &str,
) -> Result<Option<SegmentManifest>, String> {
    match connection.query_row(
        "SELECT id, conversation_id, previous_segment_id, start_reason, policy_version, bootstrap_tool_schema_digest, fixed_render_blob_id, scope_snapshot_json, forget_epoch, input_budget, last_entry_sequence
         FROM context_segments WHERE conversation_id=?1 AND status='active' ORDER BY created_at DESC LIMIT 1",
        [conversation_id],
        |row| {
            Ok(SegmentManifest {
                id: row.get(0)?,
                conversation_id: row.get(1)?,
                previous_segment_id: row.get(2)?,
                start_reason: row.get(3)?,
                policy_version: row.get(4)?,
                bootstrap_tool_schema_digest: row.get(5)?,
                fixed_render_blob_id: row.get(6)?,
                scope_snapshot_json: row.get(7)?,
                forget_epoch: row.get(8)?,
                input_budget: row.get(9)?,
                last_entry_sequence: row.get(10)?,
            })
        },
    ) {
        Ok(manifest) => Ok(Some(manifest)),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(error) => Err(error.to_string()),
    }
}

pub(crate) fn create(
    connection: &Connection,
    conversation_id: &str,
    start_reason: &str,
    previous_segment_id: Option<&str>,
    policy_version: &str,
    tool_digest: &str,
    fixed_blob_id: &str,
    scope_json: &str,
    budget: i64,
    forget_epoch: i64,
) -> Result<SegmentManifest, String> {
    let id = crate::util::new_id("segment");
    let now = crate::schedule::tick::now_ms();
    connection
        .execute(
            "INSERT INTO context_segments(
               id, conversation_id, previous_segment_id, start_reason, scope_snapshot_json,
               policy_version, bootstrap_tool_schema_digest, renderer_version, adapter_contract_version,
               fixed_render_blob_id, last_entry_sequence, input_budget, forget_epoch, created_at, status
             ) VALUES(?1,?2,?3,?4,?5,?6,?7,1,1,?8,0,?9,?10,?11,'active')",
            params![id, conversation_id, previous_segment_id, start_reason, scope_json, policy_version, tool_digest, fixed_blob_id, budget, forget_epoch, now],
        )
        .map_err(|error| error.to_string())?;
    load_active(connection, conversation_id)?.ok_or_else(|| "segment missing".into())
}

pub(crate) fn close(connection: &Connection, id: &str) -> Result<(), String> {
    connection
        .execute(
            "UPDATE context_segments SET status='closed' WHERE id=?1 AND status='active'",
            [id],
        )
        .map_err(|error| error.to_string())?;
    Ok(())
}

pub(crate) fn invalidate_for_conversation(
    connection: &Connection,
    conversation_id: &str,
    epoch: i64,
) -> Result<(), String> {
    connection
        .execute(
            "UPDATE context_segments SET status='invalidated', forget_epoch=?2 WHERE conversation_id=?1 AND status='active'",
            params![conversation_id, epoch],
        )
        .map_err(|error| error.to_string())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn db() -> Connection {
        let connection = Connection::open_in_memory().unwrap();
        crate::persistence::schema::initialize_database(&connection).unwrap();
        connection
            .execute(
                "INSERT INTO blobs(id, dedup_domain, sha256, codec, raw_bytes, stored_bytes, data, ref_count) VALUES('blob1','p','bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb','identity',1,1,X'61',1)",
                [],
            )
            .unwrap();
        connection
    }

    #[test]
    fn cw_41_create_and_load_active() {
        let connection = db();
        create(
            &connection,
            "c",
            "initial",
            None,
            "policy",
            "tools",
            "blob1",
            "{}",
            100,
            0,
        )
        .unwrap();
        let loaded = load_active(&connection, "c").unwrap().unwrap();
        assert_eq!(loaded.start_reason, "initial");
    }

    #[test]
    fn cw_41_close_then_create_links_previous() {
        let connection = db();
        let first = create(
            &connection,
            "c",
            "initial",
            None,
            "policy",
            "tools",
            "blob1",
            "{}",
            100,
            0,
        )
        .unwrap();
        close(&connection, &first.id).unwrap();
        let second = create(
            &connection,
            "c",
            "budget",
            Some(&first.id),
            "policy",
            "tools",
            "blob1",
            "{}",
            100,
            0,
        )
        .unwrap();
        assert_eq!(
            second.previous_segment_id.as_deref(),
            Some(first.id.as_str())
        );
    }
}
