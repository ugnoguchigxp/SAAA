//! Generation-flow tables (plan 12.4). Called from the existing migration transaction.

use rusqlite::Connection;

pub fn migrate(connection: &Connection) -> rusqlite::Result<()> {
    connection.execute_batch(
"         CREATE TABLE IF NOT EXISTS generated_capability_generation_jobs (
           id TEXT PRIMARY KEY,
           principal_id TEXT NOT NULL,
           conversation_id TEXT NOT NULL REFERENCES conversations(id),
           run_id TEXT NOT NULL,
           input_message_id TEXT NOT NULL,
           project_id TEXT,
           request_id TEXT NOT NULL,
           request_snapshot_json TEXT NOT NULL CHECK(json_valid(request_snapshot_json)),
           request_digest TEXT NOT NULL CHECK(length(request_digest)=64),
           capability_id TEXT NOT NULL,
           base_revision_id TEXT REFERENCES generated_capability_revisions(id),
           expected_epoch INTEGER NOT NULL CHECK(expected_epoch>=0),
           revision_id TEXT REFERENCES generated_capability_revisions(id),
           status TEXT NOT NULL CHECK(status IN ('requested','generating','building','importing',
             'verifying','awaiting_activation','active','failed','cancelled','conflict','interrupted')),
           error_code TEXT,
           usage_json TEXT CHECK(usage_json IS NULL OR json_valid(usage_json)),
           created_at INTEGER NOT NULL,
           completed_at INTEGER,
           UNIQUE(principal_id,run_id,input_message_id)
         );
         CREATE UNIQUE INDEX IF NOT EXISTS generated_generation_one_active_job
           ON generated_capability_generation_jobs(capability_id)
           WHERE status IN ('requested','generating','building','importing','verifying','awaiting_activation');
         CREATE TABLE IF NOT EXISTS generated_capability_call_owners (
           call_id TEXT PRIMARY KEY REFERENCES generated_capability_calls(id),
           principal_id TEXT NOT NULL,
           conversation_id TEXT NOT NULL REFERENCES conversations(id),
           project_id TEXT,
           run_id TEXT NOT NULL
         );",
    )
}
