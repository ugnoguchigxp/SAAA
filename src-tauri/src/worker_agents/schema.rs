//! Worker-agent tables. Idempotent `CREATE ... IF NOT EXISTS` migration (docs/plans/worker-agents.md §4.2).
//!
//! Must run after `tool_selection::schema::migrate`: `worker_profile_tools` references
//! `tool_selection_revisions`. No row here is ever pruned automatically; `worker_url_blocklist`
//! entries live until the user removes them.
use rusqlite::Connection;

pub(crate) fn migrate(connection: &Connection) -> rusqlite::Result<()> {
    connection.execute_batch(WORKER_SCHEMA)
}

const WORKER_SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS worker_meta (
  singleton INTEGER PRIMARY KEY CHECK(singleton = 1),
  registry_epoch INTEGER NOT NULL DEFAULT 0 CHECK(registry_epoch >= 0),
  acl_epoch INTEGER NOT NULL DEFAULT 0 CHECK(acl_epoch >= 0),
  web_search_mode TEXT NOT NULL DEFAULT 'inline' CHECK(web_search_mode IN ('inline','worker'))
);
INSERT OR IGNORE INTO worker_meta(singleton) VALUES (1);
CREATE TABLE IF NOT EXISTS worker_profiles (
  id TEXT PRIMARY KEY CHECK(length(id) BETWEEN 3 AND 40),
  origin TEXT NOT NULL CHECK(origin IN ('builtin','user')),
  enabled INTEGER NOT NULL CHECK(enabled IN (0,1)),
  pinned_offer INTEGER NOT NULL DEFAULT 0 CHECK(pinned_offer IN (0,1)),
  current_revision_id TEXT,
  created_at_ms INTEGER NOT NULL, updated_at_ms INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS worker_profile_revisions (
  id TEXT PRIMARY KEY,
  profile_id TEXT NOT NULL REFERENCES worker_profiles(id),
  revision INTEGER NOT NULL CHECK(revision > 0),
  review_state TEXT NOT NULL CHECK(review_state IN ('draft','approved','rejected','superseded')),
  created_by TEXT NOT NULL CHECK(created_by IN ('host_seed','user_ipc')),
  purpose TEXT NOT NULL CHECK(length(purpose) BETWEEN 1 AND 2000),
  system_context TEXT NOT NULL CHECK(length(system_context) BETWEEN 1 AND 16384),
  definition_hash TEXT NOT NULL CHECK(length(definition_hash) = 64),
  input_schema_json TEXT NOT NULL CHECK(json_valid(input_schema_json)),
  output_kind TEXT NOT NULL CHECK(output_kind IN ('web_claims_v1','json_v1')),
  output_schema_json TEXT CHECK(output_schema_json IS NULL OR json_valid(output_schema_json)),
  completion_json TEXT NOT NULL CHECK(json_valid(completion_json)),
  limits_json TEXT NOT NULL CHECK(json_valid(limits_json)),
  tier_policy_json TEXT NOT NULL CHECK(json_valid(tier_policy_json)),
  created_at_ms INTEGER NOT NULL, approved_at_ms INTEGER,
  UNIQUE(profile_id, revision), UNIQUE(profile_id, id)
);
CREATE TABLE IF NOT EXISTS worker_profile_tools (
  profile_revision_id TEXT NOT NULL REFERENCES worker_profile_revisions(id) ON DELETE CASCADE,
  ordinal INTEGER NOT NULL CHECK(ordinal >= 0),
  tool_kind TEXT NOT NULL CHECK(tool_kind IN ('builtin','catalog')),
  tool_key TEXT NOT NULL CHECK(length(tool_key) BETWEEN 1 AND 256),
  catalog_revision_id TEXT REFERENCES tool_selection_revisions(id),
  effect TEXT NOT NULL CHECK(effect IN ('pure','read','write','unknown')),
  PRIMARY KEY(profile_revision_id, tool_key),
  CHECK((tool_kind = 'catalog') = (catalog_revision_id IS NOT NULL))
);
CREATE TABLE IF NOT EXISTS worker_skills (id TEXT PRIMARY KEY, name TEXT NOT NULL CHECK(length(name) BETWEEN 1 AND 80));
CREATE TABLE IF NOT EXISTS worker_skill_revisions (
  id TEXT PRIMARY KEY, skill_id TEXT NOT NULL REFERENCES worker_skills(id),
  content_hash TEXT NOT NULL CHECK(length(content_hash) = 64),
  body TEXT NOT NULL CHECK(length(body) BETWEEN 1 AND 16384),
  created_at_ms INTEGER NOT NULL, UNIQUE(skill_id, content_hash)
);
CREATE TABLE IF NOT EXISTS worker_profile_skills (
  profile_revision_id TEXT NOT NULL REFERENCES worker_profile_revisions(id) ON DELETE CASCADE,
  ordinal INTEGER NOT NULL, skill_revision_id TEXT NOT NULL REFERENCES worker_skill_revisions(id),
  PRIMARY KEY(profile_revision_id, ordinal)
);
CREATE VIRTUAL TABLE IF NOT EXISTS worker_profile_fts USING fts5(revision_id UNINDEXED, search_text, tokenize = 'trigram');
CREATE TABLE IF NOT EXISTS worker_profile_embeddings (
  revision_id TEXT NOT NULL REFERENCES worker_profile_revisions(id) ON DELETE CASCADE,
  model_hash TEXT NOT NULL, dimension INTEGER NOT NULL CHECK(dimension > 0), vector BLOB NOT NULL,
  PRIMARY KEY(revision_id, model_hash), CHECK(length(vector) = 4 * dimension)
);
CREATE TABLE IF NOT EXISTS worker_discovery_decisions (
  id TEXT PRIMARY KEY, conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
  input_message_id TEXT REFERENCES conversation_messages(id) ON DELETE CASCADE,
  job_key TEXT NOT NULL, registry_epoch INTEGER NOT NULL, acl_epoch INTEGER NOT NULL, model_hash TEXT,
  status TEXT NOT NULL CHECK(status IN ('ok','ambiguous','no_match','degraded')),
  created_at_ms INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS worker_discovery_candidates (
  decision_id TEXT NOT NULL REFERENCES worker_discovery_decisions(id) ON DELETE CASCADE,
  profile_revision_id TEXT NOT NULL REFERENCES worker_profile_revisions(id),
  lex_rank INTEGER, vec_rank INTEGER, vec_score REAL, score REAL NOT NULL,
  final_rank INTEGER NOT NULL, offered INTEGER NOT NULL CHECK(offered IN (0,1)), pinned INTEGER NOT NULL CHECK(pinned IN (0,1)),
  PRIMARY KEY(decision_id, profile_revision_id)
);
CREATE TABLE IF NOT EXISTS worker_tasks (
  id TEXT PRIMARY KEY,
  conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
  input_message_id TEXT NOT NULL REFERENCES conversation_messages(id) ON DELETE CASCADE,
  origin_job_key TEXT NOT NULL, decision_id TEXT REFERENCES worker_discovery_decisions(id) ON DELETE SET NULL,
  profile_id TEXT NOT NULL REFERENCES worker_profiles(id),
  profile_revision_id TEXT NOT NULL REFERENCES worker_profile_revisions(id),
  idempotency_key TEXT NOT NULL UNIQUE,
  input_json TEXT NOT NULL CHECK(json_valid(input_json)),
  state TEXT NOT NULL CHECK(state IN ('accepted','running','verifying','succeeded','failed','cancelled')),
  delivery TEXT NOT NULL CHECK(delivery IN ('sync_waiting','sync_delivered','async_queued','async_delivered','suppressed')),
  tier_index INTEGER NOT NULL DEFAULT 0, attempts INTEGER NOT NULL DEFAULT 0, restarts INTEGER NOT NULL DEFAULT 0,
  deadline_at_ms INTEGER NOT NULL, sync_wait_until_ms INTEGER NOT NULL,
  result_json TEXT CHECK(result_json IS NULL OR json_valid(result_json)),
  failure_code TEXT, created_at_ms INTEGER NOT NULL, updated_at_ms INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_worker_tasks_input ON worker_tasks(input_message_id, state);
CREATE TABLE IF NOT EXISTS worker_attempts (
  task_id TEXT NOT NULL REFERENCES worker_tasks(id) ON DELETE CASCADE, ordinal INTEGER NOT NULL,
  tier TEXT NOT NULL CHECK(tier IN ('local','local_large','cloud')), route_fingerprint TEXT NOT NULL,
  status TEXT NOT NULL CHECK(status IN ('running','succeeded','failed','cancelled','interrupted')),
  error_code TEXT, steps_used INTEGER NOT NULL DEFAULT 0, started_at_ms INTEGER NOT NULL, finished_at_ms INTEGER,
  PRIMARY KEY(task_id, ordinal)
);
CREATE TABLE IF NOT EXISTS worker_tool_calls (
  task_id TEXT NOT NULL REFERENCES worker_tasks(id) ON DELETE CASCADE,
  operation_key TEXT NOT NULL,
  attempt_ordinal INTEGER NOT NULL, tool_key TEXT NOT NULL,
  effect TEXT NOT NULL CHECK(effect IN ('pure','read','write','unknown')),
  dispatch_state TEXT NOT NULL CHECK(dispatch_state IN ('reserved','dispatched','settled','unknown')),
  outcome TEXT, created_at_ms INTEGER NOT NULL, updated_at_ms INTEGER NOT NULL,
  PRIMARY KEY(task_id, operation_key)
);
CREATE TABLE IF NOT EXISTS worker_sources (
  task_id TEXT NOT NULL REFERENCES worker_tasks(id) ON DELETE CASCADE,
  url TEXT NOT NULL CHECK(length(url) <= 2048), host TEXT NOT NULL,
  kind TEXT NOT NULL CHECK(kind IN ('search_hit','fetched','user_supplied')),
  status TEXT NOT NULL CHECK(status IN ('recorded','usable','failed','excluded')),
  guard_decision TEXT, warning_categories_json TEXT NOT NULL DEFAULT '[]' CHECK(json_valid(warning_categories_json)),
  check_suspected INTEGER CHECK(check_suspected IN (0,1)),
  check_categories_json TEXT NOT NULL DEFAULT '[]' CHECK(json_valid(check_categories_json)),
  check_excerpt TEXT CHECK(check_excerpt IS NULL OR length(check_excerpt) <= 240),
  fail_reason TEXT, created_at_ms INTEGER NOT NULL,
  PRIMARY KEY(task_id, kind, url)
);
CREATE TABLE IF NOT EXISTS worker_url_blocklist (
  url_hash TEXT PRIMARY KEY CHECK(length(url_hash) = 64), host TEXT NOT NULL,
  reason TEXT NOT NULL, created_at_ms INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS worker_escalations (
  id TEXT PRIMARY KEY, task_id TEXT NOT NULL REFERENCES worker_tasks(id) ON DELETE CASCADE,
  tier TEXT NOT NULL, route_fingerprint TEXT NOT NULL, estimated_cost_micros INTEGER,
  status TEXT NOT NULL CHECK(status IN ('proposed','approved','declined','expired','consumed')),
  expires_at_ms INTEGER NOT NULL, created_at_ms INTEGER NOT NULL, decided_at_ms INTEGER
);
"#;

#[cfg(test)]
mod tests {
    use crate::worker_agents::test_support::fresh_db;

    #[test]
    fn migration_is_idempotent_and_seeds_one_meta_row() {
        let connection = fresh_db();
        super::migrate(&connection).unwrap();
        super::migrate(&connection).unwrap();
        let (epoch, mode): (i64, String) = connection
            .query_row(
                "SELECT registry_epoch, web_search_mode FROM worker_meta WHERE singleton = 1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!((epoch, mode.as_str()), (0, "inline"));
    }

    #[test]
    fn blocklist_has_no_expiry_column() {
        let connection = fresh_db();
        let columns: Vec<String> = connection
            .prepare("SELECT name FROM pragma_table_info('worker_url_blocklist')")
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert!(!columns.iter().any(|name| name.contains("expires")));
    }
}
