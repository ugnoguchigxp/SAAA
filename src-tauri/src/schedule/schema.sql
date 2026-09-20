CREATE TABLE IF NOT EXISTS schedule_payloads (
  id TEXT PRIMARY KEY,
  body TEXT,
  classification TEXT NOT NULL DEFAULT 'internal'
    CHECK(classification IN ('public','internal','confidential','restricted'))
);
CREATE TABLE IF NOT EXISTS schedule_entries (
  id TEXT PRIMARY KEY,
  kind TEXT NOT NULL CHECK(kind IN ('task_run','reminder','check_in','digest','hold_until')),
  subject_ref TEXT NOT NULL,
  scope_ref TEXT NOT NULL,
  due_at INTEGER NOT NULL,
  window_end_at INTEGER,
  status TEXT NOT NULL CHECK(status IN ('scheduled','firing','fired','missed','withdrawn','superseded')),
  origin TEXT NOT NULL CHECK(origin IN ('user_explicit','delegation','planner_candidate','user_calendar_edit')),
  delegation_ref TEXT,
  revision INTEGER NOT NULL DEFAULT 1 CHECK(revision >= 1),
  supersedes TEXT,
  created_at INTEGER NOT NULL,
  fired_at INTEGER,
  fire_result TEXT,
  payload_id TEXT REFERENCES schedule_payloads(id),
  CHECK(status != 'superseded' OR supersedes IS NOT NULL)
);
CREATE INDEX IF NOT EXISTS schedule_due ON schedule_entries(status, due_at);
CREATE TABLE IF NOT EXISTS calendar_projections (
  entry_id TEXT PRIMARY KEY REFERENCES schedule_entries(id),
  calendar_id TEXT NOT NULL,
  event_id TEXT,
  projected_rev INTEGER NOT NULL,
  projected_hash TEXT NOT NULL,
  remote_etag TEXT,
  remote_updated INTEGER,
  state TEXT NOT NULL CHECK(state IN ('pending','synced','stale','remote_deleted','error')),
  attempts INTEGER NOT NULL DEFAULT 0,
  next_attempt_at INTEGER,
  last_error TEXT
);
CREATE TABLE IF NOT EXISTS calendar_observations (
  id TEXT PRIMARY KEY,
  event_id TEXT NOT NULL,
  entry_id TEXT,
  observed_at INTEGER NOT NULL,
  remote_etag TEXT NOT NULL,
  remote_start INTEGER,
  remote_end INTEGER,
  remote_status TEXT CHECK(remote_status IS NULL OR remote_status IN ('confirmed','cancelled')),
  remote_summary TEXT,
  diff_kind TEXT NOT NULL CHECK(diff_kind IN ('moved','deleted','title_edited','foreign_event','unchanged')),
  handled TEXT NOT NULL CHECK(handled IN ('pending','applied','asked','ignored','conflict'))
);
CREATE TABLE IF NOT EXISTS calendar_sync_state (
  calendar_id TEXT PRIMARY KEY,
  sync_token TEXT,
  last_full_sync INTEGER,
  last_error TEXT
);
CREATE TABLE IF NOT EXISTS schedule_runtime (
  id INTEGER PRIMARY KEY CHECK(id=1),
  enabled INTEGER NOT NULL DEFAULT 0 CHECK(enabled IN (0,1)),
  calendar_enabled INTEGER NOT NULL DEFAULT 0 CHECK(calendar_enabled IN (0,1)),
  calendar_id TEXT,
  last_error TEXT,
  last_tick_at INTEGER,
  notify_codes TEXT NOT NULL DEFAULT '{}'
);
INSERT OR IGNORE INTO schedule_runtime(id, enabled, calendar_enabled) VALUES(1, 0, 0);
CREATE TABLE IF NOT EXISTS schedule_tombstones (
  id TEXT PRIMARY KEY,
  forgotten_at INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS schedule_notices (
  key TEXT PRIMARY KEY,
  created_at INTEGER NOT NULL
);
CREATE TRIGGER IF NOT EXISTS schedule_erase_after_tombstone AFTER INSERT ON schedule_tombstones
BEGIN
  UPDATE schedule_payloads SET body=NULL WHERE id=NEW.id;
  UPDATE calendar_observations SET remote_summary=NULL WHERE id=NEW.id;
  UPDATE calendar_projections SET state='stale', next_attempt_at=0
    WHERE entry_id IN (SELECT id FROM schedule_entries WHERE payload_id=NEW.id);
END;
