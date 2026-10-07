-- Repair only the identifiable legacy ISO-to-integer truncation. Keep the old version
-- immutable/unavailable and import the same raw bytes with the correct original time.
CREATE TEMP TABLE personal_clock_repairs AS
SELECT p.sequence,p.message_id,p.version,p.role,p.bytes,
 CAST(unixepoch(m.created_at,'subsec')*1000 AS INTEGER) AS recorded_at,
 (SELECT scope_key FROM personal_jobs j WHERE j.source_sequence=p.sequence LIMIT 1) AS scope_key
FROM personal_sources p JOIN conversation_messages m ON m.id=p.message_id
WHERE p.available=1 AND p.version=1 AND m.created_at GLOB '????-??-??T*'
 AND p.recorded_at=CAST(m.created_at AS INTEGER)
 AND unixepoch(m.created_at,'subsec') IS NOT NULL;
UPDATE personal_sources SET available=0 WHERE sequence IN (SELECT sequence FROM personal_clock_repairs);
UPDATE personal_jobs SET status='blocked',result_code='source-clock-migrated',lease_generation=lease_generation+1 WHERE source_sequence IN (SELECT sequence FROM personal_clock_repairs);
INSERT INTO personal_sources(message_id,version,role,bytes,recorded_at)
SELECT message_id,version+1,role,bytes,recorded_at FROM personal_clock_repairs;
INSERT OR IGNORE INTO personal_source_scope_refs(source_id,version,scope_key)
SELECT r.source_id,r.version+1,r.scope_key FROM personal_source_scope_refs r JOIN personal_clock_repairs p ON p.message_id=r.source_id AND p.version=r.version;
UPDATE personal_scope SET input_epoch=input_epoch+(SELECT count(*) FROM personal_clock_repairs),
 last_foreground_at=MAX(last_foreground_at,COALESCE((SELECT MAX(recorded_at) FROM personal_clock_repairs),0));
INSERT OR IGNORE INTO personal_jobs(source_sequence,epoch,status,scope_key)
SELECT s.sequence,(SELECT input_epoch FROM personal_scope),'queued',r.scope_key
FROM personal_sources s JOIN personal_clock_repairs r ON r.message_id=s.message_id AND s.version=r.version+1 AND s.available=1;
DROP TABLE personal_clock_repairs;
