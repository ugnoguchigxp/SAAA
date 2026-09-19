-- World Model projection (WM-05). Additive: the canonical history stays in the
-- existing personal_assertions / personal_transitions / personal_payloads tables.
-- All tables are re-derivable and erase on tombstone insert.

CREATE TABLE IF NOT EXISTS personal_world_projection_meta (
 id INTEGER PRIMARY KEY CHECK(id=1),
 ledger_revision INTEGER NOT NULL,
 input_epoch INTEGER NOT NULL,
 policy_revision INTEGER NOT NULL,
 built_at_ms INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS personal_world_entities (
 assertion_id TEXT PRIMARY KEY REFERENCES personal_assertions(id) ON DELETE CASCADE,
 project_scope TEXT NOT NULL,
 entity_id TEXT NOT NULL,
 kind TEXT NOT NULL,
 name TEXT NOT NULL,
 canonical_name TEXT NOT NULL,
 status TEXT NOT NULL,
 valid_from INTEGER NOT NULL,
 valid_until INTEGER,
 revision INTEGER NOT NULL,
 UNIQUE(project_scope, entity_id)
);
CREATE INDEX IF NOT EXISTS personal_world_entity_name ON personal_world_entities(project_scope, canonical_name);

CREATE TABLE IF NOT EXISTS personal_world_aliases (
 assertion_id TEXT NOT NULL REFERENCES personal_world_entities(assertion_id) ON DELETE CASCADE,
 canonical_alias TEXT NOT NULL,
 project_scope TEXT NOT NULL,
 entity_id TEXT NOT NULL,
 PRIMARY KEY(assertion_id, canonical_alias),
 FOREIGN KEY(project_scope, entity_id)
   REFERENCES personal_world_entities(project_scope, entity_id) ON DELETE CASCADE
);
CREATE INDEX IF NOT EXISTS personal_world_alias_lookup ON personal_world_aliases(project_scope, canonical_alias);

CREATE TABLE IF NOT EXISTS personal_world_relations (
 assertion_id TEXT PRIMARY KEY REFERENCES personal_assertions(id) ON DELETE CASCADE,
 project_scope TEXT NOT NULL,
 from_entity_id TEXT NOT NULL,
 to_entity_id TEXT NOT NULL,
 relation_type TEXT NOT NULL,
 status TEXT NOT NULL,
 valid_from INTEGER NOT NULL,
 valid_until INTEGER,
 revision INTEGER NOT NULL,
 FOREIGN KEY(project_scope, from_entity_id)
   REFERENCES personal_world_entities(project_scope, entity_id) ON DELETE CASCADE,
 FOREIGN KEY(project_scope, to_entity_id)
   REFERENCES personal_world_entities(project_scope, entity_id) ON DELETE CASCADE
);
CREATE INDEX IF NOT EXISTS personal_world_relation_from
 ON personal_world_relations(project_scope, from_entity_id, relation_type, assertion_id);
CREATE INDEX IF NOT EXISTS personal_world_relation_to
 ON personal_world_relations(project_scope, to_entity_id, relation_type, assertion_id);

CREATE TABLE IF NOT EXISTS personal_world_focus (
 assertion_id TEXT PRIMARY KEY REFERENCES personal_assertions(id) ON DELETE CASCADE,
 project_scope TEXT NOT NULL,
 entity_id TEXT NOT NULL,
 reason TEXT NOT NULL,
 objective_assertion_id TEXT,
 status TEXT NOT NULL,
 valid_from INTEGER NOT NULL,
 valid_until INTEGER,
 revision INTEGER NOT NULL,
 FOREIGN KEY(project_scope, entity_id)
   REFERENCES personal_world_entities(project_scope, entity_id) ON DELETE CASCADE
);
CREATE INDEX IF NOT EXISTS personal_world_focus_lookup
 ON personal_world_focus(project_scope, entity_id, reason);

-- A forget from any existing entry point wipes the whole World projection so no
-- derived name, alias, edge or Focus survives. Rebuild is the only repair path.
CREATE TRIGGER IF NOT EXISTS personal_world_forget
AFTER INSERT ON personal_tombstones
BEGIN
 DELETE FROM personal_world_aliases;
 DELETE FROM personal_world_relations;
 DELETE FROM personal_world_focus;
 DELETE FROM personal_world_entities;
 DELETE FROM personal_world_projection_meta;
END;
