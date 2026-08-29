use rusqlite::Connection;

pub fn migrate_v11_to_v12(connection: &Connection) -> rusqlite::Result<()> {
    connection.execute_batch(
        "CREATE TABLE IF NOT EXISTS continuity_state (
           id TEXT PRIMARY KEY CHECK(id = 'primary'),
           canonical_conversation_id TEXT NOT NULL,
           context_policy_version INTEGER NOT NULL CHECK(context_policy_version > 0),
           capsule_active_revision INTEGER NOT NULL DEFAULT 0 CHECK(capsule_active_revision >= 0),
           capsule_checkpoint_created_at TEXT,
           capsule_checkpoint_message_id TEXT,
           created_at TEXT NOT NULL,
           updated_at TEXT NOT NULL,
           FOREIGN KEY(canonical_conversation_id) REFERENCES conversations(id),
           CHECK(
             (capsule_checkpoint_created_at IS NULL AND capsule_checkpoint_message_id IS NULL)
             OR (capsule_checkpoint_created_at IS NOT NULL AND capsule_checkpoint_message_id IS NOT NULL)
           )
         );

         CREATE TABLE IF NOT EXISTS memory_source_windows (
           id TEXT PRIMARY KEY,
           source_ref TEXT NOT NULL UNIQUE CHECK(length(source_ref) BETWEEN 32 AND 160),
           start_message_id TEXT,
           end_message_id TEXT,
           source_digest TEXT NOT NULL CHECK(length(source_digest) = 64),
           availability TEXT NOT NULL CHECK(availability IN ('available','deleted')),
           created_at TEXT NOT NULL,
           updated_at TEXT NOT NULL,
           FOREIGN KEY(start_message_id) REFERENCES conversation_messages(id) ON DELETE SET NULL,
           FOREIGN KEY(end_message_id) REFERENCES conversation_messages(id) ON DELETE SET NULL,
           CHECK(
             (availability = 'available' AND start_message_id IS NOT NULL AND end_message_id IS NOT NULL)
             OR (availability = 'deleted' AND start_message_id IS NULL AND end_message_id IS NULL)
           )
         );
         CREATE INDEX IF NOT EXISTS idx_memory_source_windows_availability
           ON memory_source_windows(availability, updated_at);

         CREATE TRIGGER IF NOT EXISTS conversation_messages_memory_source_tombstone
         BEFORE DELETE ON conversation_messages
         BEGIN
           UPDATE memory_source_windows
           SET start_message_id = NULL,
               end_message_id = NULL,
               availability = 'deleted',
               updated_at = CAST(unixepoch('subsec') * 1000 AS INTEGER)
           WHERE start_message_id = OLD.id OR end_message_id = OLD.id;
         END;

         CREATE TABLE IF NOT EXISTS continuity_capsule_revisions (
           id TEXT PRIMARY KEY,
           revision INTEGER NOT NULL UNIQUE CHECK(revision > 0),
           status TEXT NOT NULL CHECK(status IN ('building','active','superseded','failed')),
           source_max_created_at TEXT NOT NULL,
           source_max_message_id TEXT NOT NULL,
           source_digest TEXT NOT NULL CHECK(length(source_digest) = 64),
           token_count INTEGER NOT NULL CHECK(token_count >= 0),
           created_at TEXT NOT NULL,
           activated_at TEXT
         );
         CREATE UNIQUE INDEX IF NOT EXISTS idx_continuity_capsule_one_active
           ON continuity_capsule_revisions(status) WHERE status = 'active';

         CREATE TABLE IF NOT EXISTS continuity_capsule_items (
           id TEXT PRIMARY KEY,
           revision_id TEXT NOT NULL,
           item_kind TEXT NOT NULL CHECK(item_kind IN (
             'active_referent','constraint','open_loop','commitment','recent_decision'
           )),
           semantic_key TEXT NOT NULL CHECK(length(semantic_key) BETWEEN 1 AND 128),
           value_json TEXT NOT NULL CHECK(json_valid(value_json) AND length(value_json) <= 4000),
           status TEXT NOT NULL CHECK(status IN ('active','resolved','superseded','stale')),
           priority INTEGER NOT NULL DEFAULT 0 CHECK(priority BETWEEN -100 AND 100),
           source_window_id TEXT NOT NULL,
           valid_until TEXT,
           created_at TEXT NOT NULL,
           updated_at TEXT NOT NULL,
           FOREIGN KEY(revision_id) REFERENCES continuity_capsule_revisions(id) ON DELETE CASCADE,
           FOREIGN KEY(source_window_id) REFERENCES memory_source_windows(id),
           UNIQUE(revision_id, semantic_key)
         );
         CREATE INDEX IF NOT EXISTS idx_continuity_capsule_items_projection
           ON continuity_capsule_items(revision_id, status, priority DESC);

         CREATE TABLE IF NOT EXISTS user_profile_items (
           id TEXT PRIMARY KEY,
           item_kind TEXT NOT NULL CHECK(item_kind IN ('preference','communication','accessibility')),
           semantic_key TEXT NOT NULL CHECK(length(semantic_key) BETWEEN 1 AND 128),
           value_json TEXT NOT NULL CHECK(json_valid(value_json) AND length(value_json) <= 4000),
           status TEXT NOT NULL CHECK(status IN ('candidate','active','superseded','rejected')),
           priority INTEGER NOT NULL DEFAULT 0 CHECK(priority BETWEEN -100 AND 100),
           source_window_id TEXT NOT NULL,
           created_at TEXT NOT NULL,
           updated_at TEXT NOT NULL,
           FOREIGN KEY(source_window_id) REFERENCES memory_source_windows(id)
         );
         CREATE UNIQUE INDEX IF NOT EXISTS idx_user_profile_one_active_semantic
           ON user_profile_items(semantic_key) WHERE status = 'active';
         CREATE INDEX IF NOT EXISTS idx_user_profile_review
           ON user_profile_items(status, updated_at DESC);

         CREATE TABLE IF NOT EXISTS working_state_items (
           id TEXT PRIMARY KEY,
           item_kind TEXT NOT NULL CHECK(item_kind IN ('open_loop','commitment','constraint','pending_decision')),
           semantic_key TEXT NOT NULL CHECK(length(semantic_key) BETWEEN 1 AND 128),
           value_json TEXT NOT NULL CHECK(json_valid(value_json) AND length(value_json) <= 4000),
           status TEXT NOT NULL CHECK(status IN ('active','resolved','superseded','expired')),
           priority INTEGER NOT NULL DEFAULT 0 CHECK(priority BETWEEN -100 AND 100),
           source_window_id TEXT NOT NULL,
           valid_until TEXT,
           created_at TEXT NOT NULL,
           updated_at TEXT NOT NULL,
           FOREIGN KEY(source_window_id) REFERENCES memory_source_windows(id)
         );
         CREATE UNIQUE INDEX IF NOT EXISTS idx_working_state_one_active_semantic
           ON working_state_items(semantic_key) WHERE status = 'active';
         CREATE INDEX IF NOT EXISTS idx_working_state_projection
           ON working_state_items(status, valid_until, priority DESC);

         CREATE TABLE IF NOT EXISTS memory_reflection_jobs (
           id TEXT PRIMARY KEY,
           job_kind TEXT NOT NULL CHECK(job_kind IN (
             'capsule_refresh','profile_candidate','experience_reflection',
             'working_state_cleanup','outbox_delivery'
           )),
           source_window_id TEXT,
           status TEXT NOT NULL CHECK(status IN (
             'queued','running','completed','skipped','failed','cancelled'
           )),
           attempt_count INTEGER NOT NULL DEFAULT 0 CHECK(attempt_count BETWEEN 0 AND 10),
           lease_until TEXT,
           next_attempt_at TEXT,
           result_code TEXT CHECK(result_code IS NULL OR length(result_code) <= 64),
           created_at TEXT NOT NULL,
           updated_at TEXT NOT NULL,
           UNIQUE(job_kind, source_window_id),
           FOREIGN KEY(source_window_id) REFERENCES memory_source_windows(id)
         );
         CREATE INDEX IF NOT EXISTS idx_memory_reflection_jobs_claim
           ON memory_reflection_jobs(status, next_attempt_at, created_at);

         CREATE TABLE IF NOT EXISTS memory_outbox (
           id TEXT PRIMARY KEY,
           source_window_id TEXT NOT NULL,
           idempotency_key TEXT NOT NULL UNIQUE CHECK(length(idempotency_key) = 64),
           candidate_kind TEXT NOT NULL CHECK(candidate_kind IN ('episode','profile')),
           payload_json TEXT NOT NULL CHECK(json_valid(payload_json) AND length(payload_json) <= 16000),
           abstraction_version INTEGER NOT NULL CHECK(abstraction_version > 0),
           residual_identifier_detected INTEGER NOT NULL CHECK(residual_identifier_detected = 0),
           status TEXT NOT NULL CHECK(status IN ('pending','delivered','failed','cancelled')),
           attempt_count INTEGER NOT NULL DEFAULT 0 CHECK(attempt_count BETWEEN 0 AND 10),
           receipt_digest TEXT CHECK(receipt_digest IS NULL OR length(receipt_digest) = 64),
           created_at TEXT NOT NULL,
           updated_at TEXT NOT NULL,
           FOREIGN KEY(source_window_id) REFERENCES memory_source_windows(id)
         );
         CREATE INDEX IF NOT EXISTS idx_memory_outbox_delivery
           ON memory_outbox(status, updated_at);

         CREATE TABLE IF NOT EXISTS memory_decision_events (
           id TEXT PRIMARY KEY,
           decision_kind TEXT NOT NULL CHECK(length(decision_kind) BETWEEN 1 AND 64),
           result_code TEXT NOT NULL CHECK(length(result_code) BETWEEN 1 AND 64),
           item_count INTEGER NOT NULL CHECK(item_count >= 0),
           created_at TEXT NOT NULL
         );

         CREATE TABLE IF NOT EXISTS context_projection_events (
           id TEXT PRIMARY KEY,
           health_state TEXT NOT NULL CHECK(health_state IN ('green','yellow','red')),
           projected_bytes INTEGER NOT NULL CHECK(projected_bytes >= 0),
           input_budget_bytes INTEGER NOT NULL CHECK(input_budget_bytes >= 0),
           output_reserve_bytes INTEGER NOT NULL CHECK(output_reserve_bytes >= 0),
           repair_count INTEGER NOT NULL CHECK(repair_count >= 0),
           created_at TEXT NOT NULL
         );",
    )
}
