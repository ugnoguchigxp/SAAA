CREATE TABLE IF NOT EXISTS personal_scope (
 id TEXT PRIMARY KEY CHECK(id='primary'), principal TEXT NOT NULL UNIQUE,
 revision INTEGER NOT NULL DEFAULT 0, input_epoch INTEGER NOT NULL DEFAULT 0,
 policy_revision INTEGER NOT NULL DEFAULT 1, recovery_ready INTEGER NOT NULL DEFAULT 1,
 last_foreground_at INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE IF NOT EXISTS personal_sources (
 sequence INTEGER PRIMARY KEY AUTOINCREMENT, message_id TEXT NOT NULL, version INTEGER NOT NULL,
 role TEXT NOT NULL, bytes INTEGER NOT NULL, recorded_at INTEGER NOT NULL,
 available INTEGER NOT NULL DEFAULT 1, UNIQUE(message_id,version)
);
CREATE INDEX IF NOT EXISTS personal_source_current ON personal_sources(message_id,available,version);
CREATE TABLE IF NOT EXISTS personal_tombstones (source_id TEXT PRIMARY KEY, forgotten_at INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS personal_payloads (id TEXT PRIMARY KEY,value_json TEXT NOT NULL CHECK(json_valid(value_json)),bytes INTEGER NOT NULL CHECK(bytes<=2000));
CREATE TABLE IF NOT EXISTS personal_assertions (id TEXT PRIMARY KEY,metadata TEXT NOT NULL CHECK(json_valid(metadata)),payload_id TEXT NOT NULL, erased INTEGER NOT NULL DEFAULT 0);
CREATE TABLE IF NOT EXISTS personal_transitions (sequence INTEGER PRIMARY KEY,event_id TEXT NOT NULL UNIQUE,assertion_id TEXT NOT NULL,metadata TEXT NOT NULL CHECK(json_valid(metadata)));
CREATE INDEX IF NOT EXISTS personal_transition_assertion ON personal_transitions(assertion_id,sequence);
CREATE TABLE IF NOT EXISTS personal_dependencies (
 assertion_id TEXT NOT NULL, dependency_kind TEXT NOT NULL CHECK(dependency_kind IN ('source','assertion')),
 dependency_id TEXT NOT NULL, PRIMARY KEY(assertion_id,dependency_kind,dependency_id)
);
CREATE INDEX IF NOT EXISTS personal_reverse_dependencies ON personal_dependencies(dependency_kind,dependency_id,assertion_id);
CREATE TABLE IF NOT EXISTS personal_source_refs (key_json TEXT PRIMARY KEY,metadata TEXT NOT NULL CHECK(json_valid(metadata)));
CREATE TABLE IF NOT EXISTS personal_coverage (source_key TEXT PRIMARY KEY, status TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS personal_patches (id TEXT PRIMARY KEY,digest TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS personal_projection (assertion_id TEXT PRIMARY KEY,status TEXT NOT NULL,revision INTEGER NOT NULL,evaluated_at INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS personal_jobs (
 id INTEGER PRIMARY KEY AUTOINCREMENT,source_sequence INTEGER NOT NULL UNIQUE,epoch INTEGER NOT NULL,
 status TEXT NOT NULL CHECK(status IN ('queued','running','completed','failed','blocked')),
 lease_generation INTEGER NOT NULL DEFAULT 0, lease_until INTEGER,
 attempts INTEGER NOT NULL DEFAULT 0,abort_count INTEGER NOT NULL DEFAULT 0,
 next_attempt_at INTEGER NOT NULL DEFAULT 0,offset_bytes INTEGER NOT NULL DEFAULT 0,finalizing INTEGER NOT NULL DEFAULT 0,
 result_code TEXT, FOREIGN KEY(source_sequence) REFERENCES personal_sources(sequence)
);
CREATE INDEX IF NOT EXISTS personal_job_claim ON personal_jobs(status,next_attempt_at,id);
CREATE TABLE IF NOT EXISTS personal_generations (
 id TEXT PRIMARY KEY,run_id TEXT NOT NULL,attempt_id TEXT NOT NULL UNIQUE,
 request_revision INTEGER NOT NULL,input_epoch INTEGER NOT NULL,policy_revision INTEGER NOT NULL,
 projection_revision INTEGER NOT NULL,purpose TEXT NOT NULL,
 manifest_json TEXT NOT NULL CHECK(json_valid(manifest_json)),view_id TEXT UNIQUE,
 output_allowed INTEGER NOT NULL DEFAULT 1, status TEXT NOT NULL,
 materialization TEXT NOT NULL DEFAULT 'pending', cancellation TEXT NOT NULL DEFAULT 'none'
);
CREATE INDEX IF NOT EXISTS personal_generation_run ON personal_generations(run_id,output_allowed);
CREATE TABLE IF NOT EXISTS personal_generation_inputs (
 generation_id TEXT NOT NULL,source_id TEXT NOT NULL,version INTEGER NOT NULL,
 PRIMARY KEY(generation_id,source_id,version)
);
CREATE INDEX IF NOT EXISTS personal_generation_source ON personal_generation_inputs(source_id,generation_id);
CREATE TABLE IF NOT EXISTS personal_artifacts (generation_id TEXT NOT NULL,message_id TEXT PRIMARY KEY);
CREATE INDEX IF NOT EXISTS personal_artifact_generation ON personal_artifacts(generation_id);
CREATE TABLE IF NOT EXISTS personal_registrations (
 incarnation TEXT PRIMARY KEY, source_id TEXT NOT NULL,version INTEGER NOT NULL,
 context_id TEXT NOT NULL UNIQUE, metadata TEXT NOT NULL CHECK(json_valid(metadata)),
 desired TEXT NOT NULL,observed TEXT NOT NULL,pins INTEGER NOT NULL DEFAULT 0,
 expires_at INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS personal_cleanup (
 incarnation TEXT PRIMARY KEY,source_id TEXT NOT NULL,stage TEXT NOT NULL DEFAULT 'pending',
 cancel_sent INTEGER NOT NULL DEFAULT 0,remote_stopped INTEGER NOT NULL DEFAULT 0,
 registration_absent INTEGER NOT NULL DEFAULT 0,source_absent INTEGER NOT NULL DEFAULT 0,
 snapshot_safe INTEGER NOT NULL DEFAULT 0,last_code TEXT NOT NULL DEFAULT 'pending'
);
CREATE TABLE IF NOT EXISTS personal_contract (id INTEGER PRIMARY KEY CHECK(id=1),value_json TEXT NOT NULL CHECK(json_valid(value_json)));
CREATE TRIGGER IF NOT EXISTS personal_source_insert AFTER INSERT ON conversation_messages
WHEN NEW.conversation_id='conversation_primary' AND NEW.role IN ('user','assistant','transcript')
BEGIN
 INSERT INTO personal_sources(message_id,version,role,bytes,recorded_at)
 VALUES(NEW.id,COALESCE((SELECT MAX(version)+1 FROM personal_sources WHERE message_id=NEW.id),1),NEW.role,length(CAST(NEW.content AS BLOB)),CAST(NEW.created_at AS INTEGER));
 UPDATE personal_scope SET input_epoch=input_epoch+1,last_foreground_at=CAST(NEW.created_at AS INTEGER);
 INSERT INTO personal_jobs(source_sequence,epoch,status) VALUES(last_insert_rowid(),(SELECT input_epoch FROM personal_scope),'queued');
END;
CREATE TRIGGER IF NOT EXISTS personal_source_no_resurrection BEFORE INSERT ON conversation_messages
WHEN EXISTS(SELECT 1 FROM personal_tombstones WHERE source_id=NEW.id)
BEGIN SELECT RAISE(ABORT,'personal-source-forgotten'); END;
CREATE TRIGGER IF NOT EXISTS personal_source_edit AFTER UPDATE OF content ON conversation_messages
WHEN NEW.content!=OLD.content AND EXISTS(SELECT 1 FROM personal_sources WHERE message_id=OLD.id)
BEGIN
 UPDATE personal_sources SET available=0 WHERE message_id=OLD.id;
 INSERT INTO personal_sources(message_id,version,role,bytes,recorded_at) VALUES(NEW.id,(SELECT MAX(version)+1 FROM personal_sources WHERE message_id=OLD.id),NEW.role,length(CAST(NEW.content AS BLOB)),CAST(unixepoch('subsec')*1000 AS INTEGER));
 UPDATE personal_scope SET input_epoch=input_epoch+1;
 INSERT INTO personal_jobs(source_sequence,epoch,status) VALUES(last_insert_rowid(),(SELECT input_epoch FROM personal_scope),'queued');
 UPDATE personal_generations SET output_allowed=0,cancellation='requested' WHERE id IN (SELECT generation_id FROM personal_generation_inputs WHERE source_id=OLD.id);
END;
CREATE TRIGGER IF NOT EXISTS personal_source_delete BEFORE DELETE ON conversation_messages
WHEN EXISTS(SELECT 1 FROM personal_sources WHERE message_id=OLD.id)
BEGIN
 INSERT OR IGNORE INTO personal_tombstones VALUES(OLD.id,CAST(unixepoch('subsec')*1000 AS INTEGER));
 UPDATE personal_scope SET input_epoch=input_epoch+1;
 UPDATE personal_sources SET available=0 WHERE message_id=OLD.id;
 UPDATE personal_generations SET output_allowed=0,cancellation='requested' WHERE id IN (SELECT generation_id FROM personal_generation_inputs WHERE source_id=OLD.id);
 UPDATE personal_jobs SET status='blocked',result_code='source-forgotten',lease_generation=lease_generation+1 WHERE source_sequence IN (SELECT sequence FROM personal_sources WHERE message_id=OLD.id);
 INSERT OR IGNORE INTO personal_cleanup(incarnation,source_id) SELECT incarnation,source_id FROM personal_registrations WHERE source_id=OLD.id;
 UPDATE personal_registrations SET desired='deleted' WHERE source_id=OLD.id;
 DELETE FROM personal_projection;
END;
CREATE TRIGGER IF NOT EXISTS personal_erase_payloads AFTER INSERT ON personal_tombstones
BEGIN
 UPDATE personal_assertions SET erased=1 WHERE id IN (
  WITH RECURSIVE affected(id) AS (
   SELECT assertion_id FROM personal_dependencies WHERE dependency_kind='source' AND dependency_id=NEW.source_id
   UNION SELECT d.assertion_id FROM personal_dependencies d JOIN affected a ON d.dependency_kind='assertion' AND d.dependency_id=a.id
  ) SELECT id FROM affected
 );
 DELETE FROM personal_payloads WHERE id IN (SELECT payload_id FROM personal_assertions WHERE erased=1);
 UPDATE personal_assertions SET metadata='{}' WHERE erased=1;
 UPDATE personal_transitions SET metadata=json_object('id',event_id,'sequence',sequence,'assertion_id',assertion_id,'action','invalidate','reason_code','forgotten','evidence',json('[]'),'input_dependencies',json('[]'),'recorded_at',COALESCE(json_extract(metadata,'$.recorded_at'),0)) WHERE assertion_id IN (SELECT id FROM personal_assertions WHERE erased=1);
 DELETE FROM personal_source_refs WHERE json_extract(metadata,'$.key.id')=NEW.source_id;
 UPDATE personal_generations SET output_allowed=0,manifest_json='{}',cancellation='requested' WHERE id IN (SELECT generation_id FROM personal_generation_inputs WHERE source_id=NEW.source_id);
 UPDATE personal_registrations SET metadata=json_object('sourceHandle',json_extract(metadata,'$.sourceHandle')) WHERE source_id=NEW.source_id;
 DELETE FROM conversation_messages WHERE id IN (
  WITH RECURSIVE affected(source_id) AS (
   SELECT NEW.source_id
   UNION SELECT a.message_id FROM personal_generation_inputs i JOIN affected x ON i.source_id=x.source_id JOIN personal_artifacts a ON a.generation_id=i.generation_id
  ) SELECT source_id FROM affected WHERE source_id!=NEW.source_id
 );
 UPDATE personal_sources SET available=0 WHERE NOT EXISTS(SELECT 1 FROM conversation_messages m WHERE m.id=personal_sources.message_id);
 UPDATE personal_generations SET output_allowed=0,cancellation='requested',manifest_json='{}' WHERE id IN (SELECT i.generation_id FROM personal_generation_inputs i WHERE NOT EXISTS(SELECT 1 FROM conversation_messages m WHERE m.id=i.source_id));
 UPDATE personal_patches SET digest='';
 DELETE FROM personal_projection;
END;
