CREATE TABLE IF NOT EXISTS personal_remote_operations (
 id TEXT PRIMARY KEY, kind TEXT NOT NULL, generation_id TEXT,
 subject TEXT NOT NULL, allocation TEXT NOT NULL, runtime TEXT NOT NULL,
 request_digest TEXT NOT NULL DEFAULT '',
 state TEXT NOT NULL DEFAULT 'pending', receipt TEXT NOT NULL DEFAULT '{}',
 created_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS personal_remote_generation ON personal_remote_operations(generation_id,kind);
CREATE TABLE IF NOT EXISTS personal_product_binding (
 id INTEGER PRIMARY KEY CHECK(id=1), subject TEXT NOT NULL, capability TEXT NOT NULL,
 updated_at INTEGER NOT NULL
);
CREATE TRIGGER IF NOT EXISTS personal_product_forget AFTER INSERT ON personal_tombstones
BEGIN
 UPDATE personal_remote_operations SET request_digest='',receipt=CASE WHEN kind='source' THEN json_object('sourceHandle',json_extract(receipt,'$.sourceHandle')) ELSE '{}' END
 WHERE generation_id IN (SELECT generation_id FROM personal_generation_inputs WHERE source_id=NEW.source_id)
 AND kind!='forget';
END;
CREATE TABLE IF NOT EXISTS personal_remote_results (
 id TEXT PRIMARY KEY, result TEXT NOT NULL CHECK(json_valid(result)), updated_at INTEGER NOT NULL
);
