use rusqlite::Connection;

/// v39 records store. Bodies stay immutable; forget sets `records.forget_epoch`.
pub(crate) fn migrate(connection: &Connection) -> rusqlite::Result<()> {
    connection.execute_batch(
        "CREATE TABLE IF NOT EXISTS records (
           id TEXT PRIMARY KEY,
           kind TEXT NOT NULL CHECK(kind IN ('web_search','web_search_result','web_fetch','tool_result','mcp_result','conversation_message_ref')),
           origin TEXT NOT NULL CHECK(origin IN ('user_statement','external_observation','runtime_state','derived_claim')),
           principal_id TEXT NOT NULL,
           conversation_id TEXT NOT NULL,
           run_id TEXT,
           turn_id TEXT,
           parent_execution_id TEXT,
           rank INTEGER CHECK(rank IS NULL OR rank >= 1),
           observed_at INTEGER NOT NULL,
           recorded_at INTEGER NOT NULL,
           locator_json TEXT NOT NULL CHECK(json_valid(locator_json)),
           capture_state TEXT NOT NULL CHECK(capture_state IN ('streaming','complete','partial','failed')),
           capture_reason TEXT,
           version INTEGER NOT NULL DEFAULT 1,
           existing_source_locator TEXT,
           forget_epoch INTEGER
         );
         CREATE INDEX IF NOT EXISTS idx_records_run ON records(run_id, recorded_at, id);
         CREATE INDEX IF NOT EXISTS idx_records_turn ON records(turn_id, id);
         CREATE INDEX IF NOT EXISTS idx_records_kind ON records(kind, recorded_at, id);
         CREATE INDEX IF NOT EXISTS idx_records_parent ON records(parent_execution_id, rank, id);
         CREATE INDEX IF NOT EXISTS idx_records_conversation ON records(principal_id, conversation_id, recorded_at, id);
         CREATE TABLE IF NOT EXISTS record_scopes (
           record_id TEXT NOT NULL REFERENCES records(id) ON DELETE CASCADE,
           scope_key TEXT NOT NULL,
           relation TEXT NOT NULL DEFAULT 'visible',
           policy_revision INTEGER NOT NULL DEFAULT 0,
           PRIMARY KEY(record_id, scope_key, relation)
         );
         CREATE INDEX IF NOT EXISTS idx_record_scopes_key ON record_scopes(scope_key, record_id);
         CREATE TABLE IF NOT EXISTS blobs (
           id TEXT PRIMARY KEY,
           dedup_domain TEXT NOT NULL,
           sha256 TEXT NOT NULL CHECK(length(sha256) = 64),
           codec TEXT NOT NULL CHECK(codec IN ('identity')),
           raw_bytes INTEGER NOT NULL CHECK(raw_bytes >= 0),
           stored_bytes INTEGER NOT NULL CHECK(stored_bytes >= 0),
           data BLOB,
           ref_count INTEGER NOT NULL CHECK(ref_count >= 0),
           UNIQUE(dedup_domain, sha256)
         );
         CREATE TABLE IF NOT EXISTS blob_chunks (
           blob_id TEXT NOT NULL REFERENCES blobs(id) ON DELETE CASCADE,
           sequence INTEGER NOT NULL CHECK(sequence >= 0),
           raw_offset INTEGER NOT NULL CHECK(raw_offset >= 0),
           raw_bytes INTEGER NOT NULL CHECK(raw_bytes > 0),
           codec TEXT NOT NULL CHECK(codec IN ('identity')),
           data BLOB NOT NULL,
           sha256 TEXT NOT NULL CHECK(length(sha256) = 64),
           PRIMARY KEY(blob_id, sequence)
         );
         CREATE TABLE IF NOT EXISTS record_representations (
           record_id TEXT NOT NULL REFERENCES records(id) ON DELETE CASCADE,
           name TEXT NOT NULL CHECK(name IN ('received_body','readable_text','structured_json')),
           blob_id TEXT NOT NULL REFERENCES blobs(id),
           sha256 TEXT NOT NULL CHECK(length(sha256) = 64),
           byte_length INTEGER NOT NULL CHECK(byte_length >= 0),
           parser_version TEXT NOT NULL,
           PRIMARY KEY(record_id, name)
         );
         CREATE TABLE IF NOT EXISTS record_dependencies (
           derived_record_id TEXT NOT NULL,
           source_record_id TEXT NOT NULL REFERENCES records(id),
           representation TEXT NOT NULL,
           start_byte INTEGER NOT NULL CHECK(start_byte >= 0),
           end_byte INTEGER NOT NULL CHECK(end_byte >= start_byte),
           source_hash TEXT NOT NULL,
           dependency_kind TEXT NOT NULL CHECK(dependency_kind IN ('rendering','citation','outline','tool_round')),
           PRIMARY KEY(derived_record_id, source_record_id, representation, start_byte, end_byte)
         );
         CREATE INDEX IF NOT EXISTS idx_record_dependencies_source ON record_dependencies(source_record_id, derived_record_id);
         CREATE TABLE IF NOT EXISTS record_text_chunks (
           id INTEGER PRIMARY KEY,
           record_id TEXT NOT NULL REFERENCES records(id) ON DELETE CASCADE,
           representation TEXT NOT NULL,
           start_byte INTEGER NOT NULL,
           end_byte INTEGER NOT NULL,
           text TEXT NOT NULL,
           index_version INTEGER NOT NULL
         );
         CREATE INDEX IF NOT EXISTS idx_record_text_chunks_record ON record_text_chunks(record_id, representation, start_byte);
         CREATE VIRTUAL TABLE IF NOT EXISTS record_fts USING fts5(
           text, content='record_text_chunks', content_rowid='id', tokenize='trigram'
         );
         CREATE TABLE IF NOT EXISTS record_tombstones (
           record_id TEXT PRIMARY KEY,
           forgotten_at INTEGER NOT NULL,
           forget_epoch INTEGER NOT NULL,
           reason_code TEXT NOT NULL
         );
         CREATE TRIGGER IF NOT EXISTS record_no_resurrection
         BEFORE INSERT ON records
         WHEN EXISTS(SELECT 1 FROM record_tombstones WHERE record_id = NEW.id)
         BEGIN
           SELECT RAISE(ABORT, 'record forgotten');
         END;",
    )
}
