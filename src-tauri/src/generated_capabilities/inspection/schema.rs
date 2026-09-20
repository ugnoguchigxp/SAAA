//! Inspection table (plan 12.4). Called from the existing migration transaction.

use rusqlite::Connection;

pub fn migrate(connection: &Connection) -> rusqlite::Result<()> {
    connection.execute_batch(
        "         CREATE TABLE IF NOT EXISTS generated_capability_inspections (
           id TEXT PRIMARY KEY,
           revision_id TEXT NOT NULL REFERENCES generated_capability_revisions(id),
           inspector_digest TEXT NOT NULL CHECK(length(inspector_digest)=64),
           package_hash TEXT NOT NULL CHECK(length(package_hash)=64),
           source_hash TEXT NOT NULL CHECK(length(source_hash)=64),
           program_hash TEXT NOT NULL CHECK(length(program_hash)=64),
           artifact_hash TEXT NOT NULL CHECK(length(artifact_hash)=64),
           projection_hash TEXT NOT NULL CHECK(length(projection_hash)=64),
           relative_directory TEXT NOT NULL UNIQUE,
           comparison_json TEXT NOT NULL CHECK(json_valid(comparison_json)),
           created_at INTEGER NOT NULL,
           UNIQUE(revision_id,inspector_digest)
         );
         CREATE INDEX IF NOT EXISTS idx_generated_capability_call_owners_scope
           ON generated_capability_call_owners(principal_id, conversation_id);",
    )
}
