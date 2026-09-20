use rusqlite::Connection;

/// Creates the generated-capability catalog. Called from the existing migration transaction;
/// no conversation, UI or memory table is reused as the capability source of truth.
pub fn migrate(connection: &Connection) -> rusqlite::Result<()> {
    connection.execute_batch(
        "CREATE TABLE IF NOT EXISTS generated_capabilities (
           id TEXT PRIMARY KEY,
           current_revision_id TEXT,
           catalog_epoch INTEGER NOT NULL DEFAULT 0 CHECK(catalog_epoch >= 0),
           created_at TEXT NOT NULL,
           updated_at TEXT NOT NULL,
           FOREIGN KEY(id, current_revision_id)
             REFERENCES generated_capability_revisions(capability_id, id)
         );
         CREATE TABLE IF NOT EXISTS generated_capability_revisions (
           id TEXT PRIMARY KEY,
           capability_id TEXT NOT NULL,
           package_hash TEXT NOT NULL,
           inventory_hash TEXT NOT NULL,
           contract_hash TEXT NOT NULL,
           runtime_digest TEXT NOT NULL,
           required_acceptance_hash TEXT NOT NULL,
           provenance_json TEXT NOT NULL,
           manifest_json TEXT NOT NULL,
           contract_json TEXT NOT NULL,
           metadata_json TEXT NOT NULL,
           state TEXT NOT NULL CHECK(state IN ('candidate','validated','active','suspended','retired')),
           source_hash TEXT,
           program_hash TEXT,
           artifact_hash TEXT,
           created_at TEXT NOT NULL,
           FOREIGN KEY(capability_id) REFERENCES generated_capabilities(id),
           UNIQUE(capability_id, package_hash),
           UNIQUE(capability_id, id)
         );
         CREATE UNIQUE INDEX IF NOT EXISTS idx_generated_capability_active
           ON generated_capability_revisions(capability_id) WHERE state = 'active';
         CREATE TABLE IF NOT EXISTS generated_capability_checks (
           id TEXT PRIMARY KEY,
           revision_id TEXT NOT NULL,
           runtime_digest TEXT NOT NULL,
           inventory_hash TEXT NOT NULL,
           acceptance_hash TEXT NOT NULL,
           status TEXT NOT NULL CHECK(status IN ('running','passed','failed','interrupted')),
           error_code TEXT,
           report_ref TEXT,
           started_at TEXT NOT NULL,
           completed_at TEXT,
           FOREIGN KEY(revision_id) REFERENCES generated_capability_revisions(id)
         );
         CREATE INDEX IF NOT EXISTS idx_generated_capability_checks_revision
           ON generated_capability_checks(revision_id, started_at DESC);
         CREATE TABLE IF NOT EXISTS generated_capability_calls (
           id TEXT PRIMARY KEY,
           revision_id TEXT NOT NULL,
           package_hash TEXT NOT NULL,
           origin TEXT NOT NULL CHECK(origin IN ('internal','conversation','mcp')),
           status TEXT NOT NULL CHECK(status IN ('running','succeeded','failed','cancelled','interrupted')),
           result_bool INTEGER CHECK(result_bool IS NULL OR result_bool IN (0,1)),
           error_code TEXT,
           started_at TEXT NOT NULL,
           completed_at TEXT,
           FOREIGN KEY(revision_id) REFERENCES generated_capability_revisions(id)
         );
         CREATE INDEX IF NOT EXISTS idx_generated_capability_calls_revision
           ON generated_capability_calls(revision_id, started_at DESC);
         CREATE TABLE IF NOT EXISTS generated_capability_imports (
           id TEXT PRIMARY KEY,
           status TEXT NOT NULL CHECK(status IN ('staging','completed','failed','interrupted')),
           revision_id TEXT,
           error_code TEXT,
           created_at TEXT NOT NULL,
           completed_at TEXT,
           FOREIGN KEY(revision_id) REFERENCES generated_capability_revisions(id)
         );
",
    )?;
    super::generation::schema::migrate(connection)?;
    super::inspection::schema::migrate(connection)
}
