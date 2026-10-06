-- Versioned read contract. No Episode content is written by SAAA.
CREATE TABLE IF NOT EXISTS memory_episode_grants (
 scope_key TEXT PRIMARY KEY, enabled INTEGER NOT NULL DEFAULT 0 CHECK(enabled IN (0,1)),
 revision INTEGER NOT NULL DEFAULT 1
);
CREATE TABLE IF NOT EXISTS memory_episode_scope_policies(scope_key TEXT PRIMARY KEY,revision INTEGER NOT NULL DEFAULT 1);
CREATE TABLE IF NOT EXISTS memory_episode_source_policies(source_id TEXT PRIMARY KEY,revision INTEGER NOT NULL DEFAULT 1);
DROP TRIGGER IF EXISTS episode_scope_epoch_changed;
DROP TRIGGER IF EXISTS episode_scope_state_changed;
DROP TRIGGER IF EXISTS episode_scope_created;
DROP TRIGGER IF EXISTS episode_scope_removed;
CREATE TABLE IF NOT EXISTS memory_episode_changes (
 sequence INTEGER PRIMARY KEY AUTOINCREMENT, source_id TEXT NOT NULL
);
CREATE TRIGGER IF NOT EXISTS episode_source_created AFTER INSERT ON personal_sources
BEGIN INSERT INTO memory_episode_changes(source_id) VALUES(NEW.message_id); END;
CREATE TRIGGER IF NOT EXISTS episode_source_changed AFTER UPDATE OF available ON personal_sources
WHEN OLD.available!=NEW.available
BEGIN INSERT INTO memory_episode_changes(source_id) VALUES(NEW.message_id); END;
CREATE TRIGGER IF NOT EXISTS episode_grant_changed AFTER UPDATE ON memory_episode_grants
BEGIN
 INSERT INTO memory_episode_changes(source_id) SELECT DISTINCT message_id FROM personal_sources;
END;
CREATE TRIGGER IF NOT EXISTS episode_grant_created AFTER INSERT ON memory_episode_grants
BEGIN
 INSERT INTO memory_episode_changes(source_id) SELECT DISTINCT message_id FROM personal_sources;
END;
CREATE TRIGGER IF NOT EXISTS episode_scope_created AFTER INSERT ON conversation_message_scopes
BEGIN
 INSERT INTO memory_episode_source_policies(source_id) VALUES(NEW.message_id)
 ON CONFLICT(source_id) DO UPDATE SET revision=revision+1;
 INSERT INTO memory_episode_changes(source_id) VALUES(NEW.message_id);
END;
CREATE TRIGGER IF NOT EXISTS episode_scope_removed AFTER DELETE ON conversation_message_scopes
BEGIN
 INSERT INTO memory_episode_source_policies(source_id) VALUES(OLD.message_id)
 ON CONFLICT(source_id) DO UPDATE SET revision=revision+1;
 INSERT INTO memory_episode_changes(source_id) VALUES(OLD.message_id);
END;
CREATE TRIGGER IF NOT EXISTS episode_scope_state_changed AFTER UPDATE OF state ON context_scopes
WHEN NEW.state!=OLD.state
BEGIN
 INSERT INTO memory_episode_scope_policies(scope_key,revision) VALUES(NEW.scope_key,2)
 ON CONFLICT(scope_key) DO UPDATE SET revision=revision+1;
 INSERT INTO memory_episode_changes(source_id) SELECT DISTINCT message_id FROM conversation_message_scopes WHERE scope_key=NEW.scope_key;
 INSERT INTO memory_episode_changes(source_id) SELECT DISTINCT message_id FROM personal_sources
 WHERE NEW.scope_key='user:'||(SELECT principal FROM personal_scope WHERE id='primary');
END;
-- A typed correction retires the old derived evidence while keeping raw history.
CREATE TABLE IF NOT EXISTS memory_episode_retired_sources (
 source_id TEXT NOT NULL,version INTEGER NOT NULL,PRIMARY KEY(source_id,version)
);
CREATE TRIGGER IF NOT EXISTS memory_episode_retire_changed_value AFTER INSERT ON personal_transitions
WHEN json_type(NEW.metadata,'$.action.supersede')='object'
AND EXISTS(
 SELECT 1 FROM personal_assertions old JOIN personal_payloads op ON op.id=old.payload_id
 JOIN personal_assertions fresh ON fresh.id=json_extract(NEW.metadata,'$.action.supersede.by')
 JOIN personal_payloads np ON np.id=fresh.payload_id
 WHERE old.id=NEW.assertion_id AND json_extract(old.metadata,'$.kind') IN ('preference','habit','personal_fact')
 AND COALESCE(json_extract(op.value_json,'$.value'),op.value_json)!=COALESCE(json_extract(np.value_json,'$.value'),np.value_json)
)
BEGIN
 INSERT OR IGNORE INTO memory_episode_retired_sources(source_id,version)
 SELECT json_extract(e.value,'$.id'),json_extract(e.value,'$.version') FROM personal_assertions a,json_each(a.metadata,'$.evidence') e WHERE a.id=NEW.assertion_id;
 INSERT INTO memory_episode_changes(source_id)
 SELECT json_extract(e.value,'$.id') FROM personal_assertions a,json_each(a.metadata,'$.evidence') e WHERE a.id=NEW.assertion_id;
END;
DROP VIEW IF EXISTS memory_episode_source_v1;
CREATE VIEW memory_episode_source_v1 AS
SELECT p.message_id AS source_id,p.version,p.sequence,p.role,m.content,
 p.recorded_at,CAST(m.created_at AS INTEGER) AS uttered_at,
 s.principal,g.scope_key,g.revision AS policy_revision,COALESCE(sp.revision,1) AS scope_epoch,COALESCE(mp.revision,0) AS access_revision
FROM personal_sources p JOIN conversation_messages m ON m.id=p.message_id
JOIN personal_scope s ON s.id='primary' JOIN memory_episode_grants g ON g.enabled=1
JOIN context_scopes cs ON cs.scope_key=g.scope_key AND cs.state='active'
LEFT JOIN memory_episode_scope_policies sp ON sp.scope_key=cs.scope_key
LEFT JOIN memory_episode_source_policies mp ON mp.source_id=p.message_id
WHERE p.available=1 AND p.bytes>0
AND NOT EXISTS(SELECT 1 FROM personal_tombstones t WHERE t.source_id=p.message_id)
AND NOT EXISTS(SELECT 1 FROM memory_episode_retired_sources r WHERE r.source_id=p.message_id AND r.version=p.version)
AND (g.scope_key NOT LIKE 'user:%' OR NOT EXISTS(
 SELECT 1 FROM conversation_message_scopes x JOIN context_scopes sx ON sx.scope_key=x.scope_key
 WHERE x.message_id=p.message_id AND x.relation='focus' AND sx.kind NOT IN ('user','request')))
AND (EXISTS(SELECT 1 FROM conversation_message_scopes x WHERE x.message_id=p.message_id AND x.scope_key=g.scope_key)
 OR (g.scope_key='user:'||s.principal AND NOT EXISTS(SELECT 1 FROM conversation_message_scopes x WHERE x.message_id=p.message_id)));

CREATE TABLE IF NOT EXISTS memory_episode_run_refs (
 run_id TEXT NOT NULL,episode_id TEXT NOT NULL,source_key TEXT NOT NULL,contract TEXT NOT NULL CHECK(json_valid(contract)),
 PRIMARY KEY(run_id,episode_id)
);
CREATE TABLE IF NOT EXISTS memory_episode_artifacts (
 run_id TEXT NOT NULL,message_id TEXT PRIMARY KEY
);
CREATE TRIGGER IF NOT EXISTS memory_episode_artifact_forget AFTER INSERT ON personal_tombstones
BEGIN
 DELETE FROM conversation_messages WHERE id IN (
  SELECT a.message_id FROM memory_episode_artifacts a JOIN memory_episode_run_refs r ON a.run_id=r.run_id
  WHERE EXISTS(SELECT 1 FROM json_each(r.contract,'$.sources') WHERE json_extract(value,'$.sourceId')=NEW.source_id)
 );
END;
