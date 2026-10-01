-- Execution is owned by personal_state/jobs and the existing scheduler/SqliteWriter.
CREATE TABLE IF NOT EXISTS personal_review_settings (
 id INTEGER PRIMARY KEY CHECK(id=1), mode TEXT NOT NULL DEFAULT 'preview' CHECK(mode IN ('off','preview','apply')),
 dispatch_turn INTEGER NOT NULL DEFAULT 0, turn INTEGER NOT NULL DEFAULT 0
);
INSERT OR IGNORE INTO personal_review_settings(id) VALUES(1);
CREATE TABLE IF NOT EXISTS personal_review_cursor (
 scope_key TEXT PRIMARY KEY, sequence INTEGER NOT NULL DEFAULT 0, policy_revision INTEGER NOT NULL DEFAULT 1
);
CREATE TABLE IF NOT EXISTS personal_review_work (
 id INTEGER PRIMARY KEY AUTOINCREMENT, scope_key TEXT NOT NULL, sequence INTEGER NOT NULL,
 boundary INTEGER NOT NULL, scope_epoch INTEGER NOT NULL, policy_revision INTEGER NOT NULL,
 version INTEGER NOT NULL DEFAULT 1, lane TEXT NOT NULL DEFAULT 'history' CHECK(lane IN ('current','history')), status TEXT NOT NULL DEFAULT 'queued',
 generation INTEGER NOT NULL DEFAULT 0, lease_until INTEGER, next_attempt_at INTEGER NOT NULL DEFAULT 0,
 retries INTEGER NOT NULL DEFAULT 0, proposal TEXT, manifest TEXT,
 result TEXT NOT NULL DEFAULT 'queued', updated_at INTEGER NOT NULL DEFAULT 0,
 UNIQUE(scope_key,sequence,version,lane)
);
CREATE INDEX IF NOT EXISTS personal_review_claim ON personal_review_work(status,next_attempt_at,id);
CREATE TABLE IF NOT EXISTS personal_review_inputs (
 work_id INTEGER NOT NULL REFERENCES personal_review_work(id) ON DELETE CASCADE,
 source_id TEXT NOT NULL, version INTEGER NOT NULL, PRIMARY KEY(work_id,source_id,version)
);
CREATE INDEX IF NOT EXISTS personal_review_source ON personal_review_inputs(source_id,work_id);
CREATE TABLE IF NOT EXISTS personal_world_source_receipts (
 scope_key TEXT NOT NULL, source_id TEXT NOT NULL, version INTEGER NOT NULL,
 PRIMARY KEY(scope_key,source_id,version)
);
CREATE TRIGGER IF NOT EXISTS personal_review_forget AFTER INSERT ON personal_tombstones
BEGIN
 DELETE FROM personal_review_work WHERE id IN (SELECT work_id FROM personal_review_inputs WHERE source_id=NEW.source_id);
 DELETE FROM personal_world_source_receipts WHERE source_id=NEW.source_id;
END;
CREATE TRIGGER IF NOT EXISTS personal_review_edit AFTER UPDATE OF content ON conversation_messages
WHEN NEW.content!=OLD.content
BEGIN
 DELETE FROM personal_review_work WHERE id IN (SELECT work_id FROM personal_review_inputs WHERE source_id=NEW.id);
 DELETE FROM personal_world_source_receipts WHERE source_id=NEW.id;
END;
CREATE TRIGGER IF NOT EXISTS personal_review_scope AFTER UPDATE OF state ON context_scopes
WHEN NEW.state!='active'
BEGIN
 DELETE FROM personal_review_work WHERE scope_key=NEW.scope_key;
 DELETE FROM personal_world_source_receipts WHERE scope_key=NEW.scope_key;
 DELETE FROM personal_review_cursor WHERE scope_key=NEW.scope_key;
END;
CREATE TABLE IF NOT EXISTS personal_review_premises (
 work_id INTEGER NOT NULL REFERENCES personal_review_work(id) ON DELETE CASCADE,
 assertion_id TEXT NOT NULL, PRIMARY KEY(work_id,assertion_id)
);
CREATE INDEX IF NOT EXISTS personal_review_premise ON personal_review_premises(assertion_id,work_id);
CREATE TRIGGER IF NOT EXISTS personal_review_initial_input AFTER INSERT ON personal_review_work
BEGIN
 INSERT OR IGNORE INTO personal_review_inputs SELECT NEW.id,message_id,version FROM personal_sources WHERE sequence=NEW.sequence;
END;
-- Only the changed, previously supplied premise wakes a held or preview proposal.
CREATE TRIGGER IF NOT EXISTS personal_review_premise_changed AFTER INSERT ON personal_transitions
BEGIN
 UPDATE personal_review_work SET status='queued',proposal=NULL,manifest=NULL,next_attempt_at=0,
 generation=generation+1,result='premise-changed'
 WHERE status IN ('held','preview') AND id IN (
  SELECT work_id FROM personal_review_premises WHERE assertion_id=NEW.assertion_id
 );
END;
