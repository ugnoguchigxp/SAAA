DROP TRIGGER IF EXISTS personal_rebuild_after_forget;
DROP TRIGGER IF EXISTS personal_rebuild_after_edit;
DROP TRIGGER IF EXISTS personal_retire_superseded_on_forget;
CREATE TABLE IF NOT EXISTS personal_snapshots (
 scope_key TEXT NOT NULL, category TEXT NOT NULL, revision INTEGER NOT NULL,
 body TEXT NOT NULL, digest TEXT NOT NULL, manifest TEXT NOT NULL CHECK(json_valid(manifest)),
 published_at INTEGER NOT NULL, PRIMARY KEY(scope_key,category)
);
CREATE TRIGGER IF NOT EXISTS personal_snapshot_forget AFTER INSERT ON personal_tombstones
BEGIN
 DELETE FROM personal_snapshots WHERE EXISTS(
  SELECT 1 FROM json_each(personal_snapshots.manifest,'$.inputs')
  WHERE json_extract(value,'$.id')=NEW.source_id
 );
END;

CREATE TABLE IF NOT EXISTS personal_consolidation_settings (
 id INTEGER PRIMARY KEY CHECK(id=1), enabled INTEGER NOT NULL DEFAULT 0 CHECK(enabled IN (0,1))
);
INSERT OR IGNORE INTO personal_consolidation_settings(id) VALUES(1);

-- Rebuild surviving evidence before the forget trigger erases assertion metadata.
CREATE TRIGGER IF NOT EXISTS personal_rebuild_after_forget BEFORE INSERT ON personal_tombstones
BEGIN
 UPDATE personal_jobs SET status='queued',stage='continuity',offset_bytes=0,finalizing=0,
 attempts=0,next_attempt_at=0,lease_until=NULL,lease_generation=lease_generation+1
 WHERE source_sequence IN (
  SELECT p.sequence FROM personal_assertions a
  JOIN personal_dependencies d ON d.assertion_id=a.id AND d.dependency_kind='source' AND d.dependency_id=NEW.source_id
  JOIN json_each(a.metadata,'$.evidence') evidence
  JOIN personal_sources p ON p.message_id=json_extract(evidence.value,'$.id') AND p.version=json_extract(evidence.value,'$.version')
  JOIN conversation_messages m ON m.id=p.message_id
  WHERE a.erased=0 AND p.available=1 AND p.message_id!=NEW.source_id
    AND NOT EXISTS(SELECT 1 FROM personal_transitions t WHERE t.assertion_id=a.id AND
      (json_type(t.metadata,'$.action.supersede')='object' OR json_extract(t.metadata,'$.action') IN ('retract','invalidate')))
 );
END;
CREATE TRIGGER IF NOT EXISTS personal_rebuild_after_edit AFTER UPDATE OF available ON personal_sources
WHEN OLD.available=1 AND NEW.available=0
BEGIN
 UPDATE personal_jobs SET status='queued',stage='continuity',offset_bytes=0,finalizing=0,
 attempts=0,next_attempt_at=0,lease_until=NULL,lease_generation=lease_generation+1
 WHERE source_sequence IN (
  SELECT p.sequence FROM personal_assertions a
  JOIN personal_dependencies d ON d.assertion_id=a.id AND d.dependency_kind='source' AND d.dependency_id=NEW.message_id
  JOIN json_each(a.metadata,'$.evidence') evidence
  JOIN personal_sources p ON p.message_id=json_extract(evidence.value,'$.id') AND p.version=json_extract(evidence.value,'$.version')
  JOIN conversation_messages m ON m.id=p.message_id
  WHERE a.erased=0 AND p.available=1 AND p.message_id!=NEW.message_id
    AND NOT EXISTS(SELECT 1 FROM personal_transitions t WHERE t.assertion_id=a.id AND
      (json_type(t.metadata,'$.action.supersede')='object' OR json_extract(t.metadata,'$.action') IN ('retract','invalidate')))
 );
END;

CREATE TRIGGER IF NOT EXISTS personal_retire_superseded_on_forget BEFORE INSERT ON personal_tombstones
BEGIN
 UPDATE personal_transitions SET metadata=json_set(metadata,
   '$.action','invalidate','$.reason_code','superseded-source-forgotten',
   '$.evidence',json('[]'),'$.input_dependencies',json('[]'))
 WHERE json_type(metadata,'$.action.supersede')='object' AND EXISTS(
   SELECT 1 FROM json_each(personal_transitions.metadata,'$.input_dependencies')
   WHERE json_extract(value,'$.id')=NEW.source_id
 );
END;

CREATE TABLE IF NOT EXISTS personal_assertion_scope_policies(assertion_id TEXT PRIMARY KEY,scope_key TEXT NOT NULL,revision INTEGER NOT NULL);

-- Versions survive deletion of a published body; history contains no forgotten text.
CREATE TABLE IF NOT EXISTS personal_snapshot_versions (
 scope_key TEXT NOT NULL,category TEXT NOT NULL,revision INTEGER NOT NULL,
 PRIMARY KEY(scope_key,category)
);
INSERT INTO personal_snapshot_versions SELECT scope_key,category,revision FROM personal_snapshots WHERE true
ON CONFLICT(scope_key,category) DO UPDATE SET revision=max(revision,excluded.revision);
