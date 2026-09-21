use rusqlite::Connection;

pub(crate) fn migrate(connection: &Connection) -> rusqlite::Result<()> {
    connection.execute_batch(
        "CREATE TABLE IF NOT EXISTS rr_learning_jobs (
           id TEXT PRIMARY KEY, job_key TEXT NOT NULL UNIQUE,
           stage TEXT NOT NULL CHECK(stage IN ('extract','label','assemble','evaluate','publish_shadow_manifest')),
           status TEXT NOT NULL CHECK(status IN ('queued','running','paused','completed','failed')),
           upper_seq INTEGER NOT NULL CHECK(upper_seq >= 0), cursor_seq INTEGER NOT NULL CHECK(cursor_seq >= 0 AND cursor_seq <= upper_seq),
           dataset_id TEXT, error_code TEXT, created_at_ms INTEGER NOT NULL, updated_at_ms INTEGER NOT NULL,
           FOREIGN KEY(dataset_id) REFERENCES rr_datasets(id)
         );
         CREATE INDEX IF NOT EXISTS idx_rr_learning_jobs_status ON rr_learning_jobs(status, updated_at_ms);
         CREATE TABLE IF NOT EXISTS rr_learning_dirty (
           root_id TEXT PRIMARY KEY, cause_seq INTEGER NOT NULL CHECK(cause_seq > 0),
           FOREIGN KEY(root_id) REFERENCES rr_roots(root_id) ON DELETE CASCADE
         );
         CREATE TABLE IF NOT EXISTS rr_datasets (
           id TEXT PRIMARY KEY, upper_seq INTEGER NOT NULL CHECK(upper_seq >= 0), feature_version TEXT NOT NULL,
           labeler_version TEXT NOT NULL, manifest_json TEXT NOT NULL CHECK(json_valid(manifest_json)), digest TEXT,
           state TEXT NOT NULL CHECK(state IN ('building','ready','invalidated','failed')), created_at_ms INTEGER NOT NULL
         );
         CREATE TABLE IF NOT EXISTS rr_examples (
           id TEXT PRIMARY KEY, dataset_id TEXT NOT NULL, decision_id TEXT NOT NULL, label_revision INTEGER NOT NULL CHECK(label_revision > 0),
           feature_version TEXT NOT NULL, labeler_version TEXT NOT NULL, features_json TEXT NOT NULL CHECK(json_valid(features_json)),
           labels_json TEXT NOT NULL CHECK(json_valid(labels_json)), eligible INTEGER NOT NULL CHECK(eligible IN (0,1)),
           exclusion_reason TEXT, created_at_ms INTEGER NOT NULL,
           UNIQUE(dataset_id,decision_id,label_revision,feature_version,labeler_version),
           FOREIGN KEY(dataset_id) REFERENCES rr_datasets(id) ON DELETE CASCADE,
           FOREIGN KEY(decision_id) REFERENCES rr_decisions(id) ON DELETE CASCADE
         );
         CREATE TABLE IF NOT EXISTS rr_example_sources (
           example_id TEXT NOT NULL, source_kind TEXT NOT NULL, source_id TEXT NOT NULL, source_version TEXT NOT NULL, scope_key TEXT NOT NULL,
           PRIMARY KEY(example_id,source_kind,source_id,source_version,scope_key), FOREIGN KEY(example_id) REFERENCES rr_examples(id) ON DELETE CASCADE
         );
         CREATE INDEX IF NOT EXISTS idx_rr_example_sources_source ON rr_example_sources(source_kind, source_id);
         CREATE TABLE IF NOT EXISTS rr_ranker_artifacts (
           id TEXT PRIMARY KEY, dataset_id TEXT NOT NULL, algorithm TEXT NOT NULL CHECK(algorithm IN ('rules-v1','empirical-v1','linear-v1')),
           feature_version TEXT NOT NULL, candidate_fingerprint TEXT NOT NULL, weights_json TEXT NOT NULL CHECK(json_valid(weights_json)),
           metrics_json TEXT NOT NULL CHECK(json_valid(metrics_json)), digest TEXT NOT NULL, state TEXT NOT NULL CHECK(state IN ('candidate','shadow','invalidated','retired')),
           created_at_ms INTEGER NOT NULL, FOREIGN KEY(dataset_id) REFERENCES rr_datasets(id) ON DELETE CASCADE
         );
         CREATE TABLE IF NOT EXISTS rr_cleanup_journal (
           dataset_id TEXT PRIMARY KEY, state TEXT NOT NULL CHECK(state IN ('pending','completed')),
           attempts INTEGER NOT NULL DEFAULT 0 CHECK(attempts >= 0), last_error_code TEXT,
           updated_at_ms INTEGER NOT NULL, FOREIGN KEY(dataset_id) REFERENCES rr_datasets(id) ON DELETE CASCADE
         );
         CREATE TABLE IF NOT EXISTS rr_learning_runs (
           day_key TEXT PRIMARY KEY, status TEXT NOT NULL CHECK(status IN ('running','paused','completed','failed')),
           reason TEXT NOT NULL CHECK(reason IN ('window','missed_window','manual')),
           started_at_ms INTEGER NOT NULL, completed_at_ms INTEGER, error_code TEXT
         );
         CREATE TABLE IF NOT EXISTS rr_shadow_observations (
           decision_id TEXT PRIMARY KEY, artifact_id TEXT NOT NULL, scores_json TEXT NOT NULL CHECK(json_valid(scores_json)),
           rules_id TEXT NOT NULL, recommended_id TEXT NOT NULL, created_at_ms INTEGER NOT NULL,
           FOREIGN KEY(decision_id) REFERENCES rr_decisions(id) ON DELETE CASCADE,
           FOREIGN KEY(artifact_id) REFERENCES rr_ranker_artifacts(id)
         );",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rr_27_learning_schema_is_idempotent() {
        let c = Connection::open_in_memory().expect("db");
        c.execute_batch("CREATE TABLE rr_roots(root_id TEXT PRIMARY KEY);CREATE TABLE rr_decisions(id TEXT PRIMARY KEY);").expect("base");
        migrate(&c).expect("first");
        migrate(&c).expect("second");
        let tables: i64 = c.query_row("SELECT count(*) FROM sqlite_master WHERE type='table' AND name IN ('rr_learning_jobs','rr_datasets','rr_examples','rr_ranker_artifacts')",[],|r|r.get(0)).expect("tables");
        assert_eq!(tables, 4);
    }
}
