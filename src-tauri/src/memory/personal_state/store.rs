use super::{decode, encode, sources};
use crate::database_error;
use rusqlite::{params, Connection};
use saaa_personal_state_core::*;
use std::collections::{BTreeMap, BTreeSet};

fn rows(c: &Connection, sql: &str) -> Result<Vec<String>, String> {
    let mut s = c.prepare(sql).map_err(database_error)?;
    let result = s
        .query_map([], |r| r.get(0))
        .map_err(database_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(database_error);
    result
}
pub fn load(c: &Connection) -> Result<Ledger, String> {
    let mut ledger=c.query_row("SELECT principal,revision,input_epoch,policy_revision,recovery_ready FROM personal_scope WHERE id='primary'",[],|r|{let ready:bool=r.get(4)?;if !ready{return Err(rusqlite::Error::InvalidQuery)};let mut l=Ledger::new(r.get(0)?,"primary".into(),r.get(3)?);l.revision=r.get(1)?;l.input_epoch=r.get(2)?;Ok(l)}).map_err(|_|"personal-recovery-incomplete")?;
    for s in rows(c, "SELECT metadata FROM personal_source_refs")? {
        let mut source: SourceRef = decode(s)?;
        source.available=c.query_row("SELECT EXISTS(SELECT 1 FROM personal_sources WHERE sequence=?1 AND message_id=?2 AND version=?3 AND available=1)",params![source.sequence,source.key.id,source.key.version],|r|r.get(0)).map_err(database_error)?;
        ledger.sources.insert(source.key.clone(), source);
    }
    for id in rows(c, "SELECT source_id FROM personal_tombstones")? {
        ledger.tombstones.insert(id);
    }
    for s in rows(c, "SELECT metadata FROM personal_assertions WHERE erased=0")? {
        let a: Assertion = decode(s)?;
        ledger.assertions.insert(a.id.clone(), a);
    }
    for s in rows(
        c,
        "SELECT metadata FROM personal_transitions ORDER BY sequence",
    )? {
        ledger.transitions.push(decode(s)?);
    }
    let mut stmt = c
        .prepare("SELECT source_key,status FROM personal_coverage")
        .map_err(database_error)?;
    let records = stmt
        .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
        .map_err(database_error)?;
    for r in records {
        let (k, v) = r.map_err(database_error)?;
        ledger.coverage.insert(decode(k)?, decode(v)?);
    }
    let mut stmt = c
        .prepare("SELECT id,digest FROM personal_patches")
        .map_err(database_error)?;
    ledger.applied_patches = stmt
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .map_err(database_error)?
        .collect::<Result<BTreeMap<_, _>, _>>()
        .map_err(database_error)?;
    Ok(ledger)
}

/// Load only the assertion dependency closure needed by one World query.
///
/// Interactive World reads must not scale with unrelated projects, coverage
/// records or historical patch receipts. The projection tables provide the
/// project roots; assertion dependencies, transitions and source references
/// are then loaded transitively for those roots only.
pub fn load_world_query(
    c: &Connection,
    project_scope: &str,
    extra_sources: &BTreeSet<SourceKey>,
) -> Result<Ledger, String> {
    let mut ledger = c
        .query_row(
            "SELECT principal,revision,input_epoch,policy_revision,recovery_ready
               FROM personal_scope WHERE id='primary'",
            [],
            |r| {
                let ready: bool = r.get(4)?;
                if !ready {
                    return Err(rusqlite::Error::InvalidQuery);
                }
                let mut value = Ledger::new(r.get(0)?, "primary".into(), r.get(3)?);
                value.revision = r.get(1)?;
                value.input_epoch = r.get(2)?;
                Ok(value)
            },
        )
        .map_err(|_| "personal-recovery-incomplete")?;

    let mut statement = c
        .prepare(
            "WITH RECURSIVE roots(id) AS (
                 SELECT assertion_id FROM personal_world_entities WHERE project_scope=?1
                 UNION SELECT assertion_id FROM personal_world_relations WHERE project_scope=?1
                 UNION SELECT assertion_id FROM personal_world_focus WHERE project_scope=?1
                 UNION SELECT id FROM personal_assertions
                       WHERE erased=0
                         AND json_extract(metadata,'$.kind')='objective'
                         AND json_extract(metadata,'$.access.task_request')=?1
             ), closure(id) AS (
                 SELECT id FROM roots
                 UNION
                 SELECT dependency.dependency_id
                   FROM personal_dependencies dependency
                   JOIN closure parent ON parent.id=dependency.assertion_id
                  WHERE dependency.dependency_kind='assertion'
             )
             SELECT assertion.metadata
               FROM personal_assertions assertion
               JOIN closure ON closure.id=assertion.id
              WHERE assertion.erased=0",
        )
        .map_err(database_error)?;
    let assertions = statement
        .query_map([project_scope], |row| row.get::<_, String>(0))
        .map_err(database_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(database_error)?;
    for raw in assertions {
        let assertion: Assertion = decode(raw)?;
        ledger.assertions.insert(assertion.id.clone(), assertion);
    }

    let assertion_ids: Vec<String> = ledger.assertions.keys().cloned().collect();
    let assertion_ids_json = serde_json::to_string(&assertion_ids).map_err(|e| e.to_string())?;
    let mut statement = c
        .prepare(
            "SELECT metadata FROM personal_transitions
              WHERE assertion_id IN (SELECT value FROM json_each(?1))
              ORDER BY sequence",
        )
        .map_err(database_error)?;
    let transitions = statement
        .query_map([assertion_ids_json], |row| row.get::<_, String>(0))
        .map_err(database_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(database_error)?;
    for raw in transitions {
        ledger.transitions.push(decode(raw)?);
    }

    let mut source_keys = extra_sources.clone();
    for assertion in ledger.assertions.values() {
        source_keys.extend(assertion.evidence.iter().cloned());
        source_keys.extend(assertion.input_dependencies.iter().cloned());
    }
    for transition in &ledger.transitions {
        source_keys.extend(transition.evidence.iter().cloned());
        source_keys.extend(transition.input_dependencies.iter().cloned());
    }
    let encoded_keys: Vec<String> = source_keys.iter().map(encode).collect::<Result<_, _>>()?;
    let encoded_keys_json = serde_json::to_string(&encoded_keys).map_err(|e| e.to_string())?;
    let mut statement = c
        .prepare(
            "SELECT reference.metadata,
                    EXISTS(SELECT 1 FROM personal_sources source
                            WHERE source.sequence=json_extract(reference.metadata,'$.sequence')
                              AND source.message_id=json_extract(reference.metadata,'$.key.id')
                              AND source.version=json_extract(reference.metadata,'$.key.version')
                              AND source.available=1)
               FROM personal_source_refs reference
              WHERE reference.key_json IN (SELECT value FROM json_each(?1))",
        )
        .map_err(database_error)?;
    let sources = statement
        .query_map([encoded_keys_json], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, bool>(1)?))
        })
        .map_err(database_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(database_error)?;
    for (raw, available) in sources {
        let mut source: SourceRef = decode(raw)?;
        source.available = available;
        ledger.sources.insert(source.key.clone(), source);
    }

    let source_ids: Vec<String> = source_keys.iter().map(|key| key.id.clone()).collect();
    let source_ids_json = serde_json::to_string(&source_ids).map_err(|e| e.to_string())?;
    let mut statement = c
        .prepare(
            "SELECT source_id FROM personal_tombstones
              WHERE source_id IN (SELECT value FROM json_each(?1))",
        )
        .map_err(database_error)?;
    let tombstones = statement
        .query_map([source_ids_json], |row| row.get::<_, String>(0))
        .map_err(database_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(database_error)?;
    ledger.tombstones.extend(tombstones);
    Ok(ledger)
}

/// Must run inside the caller's writer transaction. All payload writes roll back
/// if any source/lease/patch fails, and no network is performed here.
/// Commit a patch. If the caller has not already opened a transaction (e.g. the
/// worker's `write_transaction`), this opens one so payload, assertion,
/// transition, revision and projection writes roll back atomically on any
/// failure (C5/§6).
pub fn commit(
    c: &Connection,
    patch: &StatePatch,
    context: &CommitContext<'_>,
    payloads: &BTreeMap<String, serde_json::Value>,
) -> Result<bool, String> {
    if c.is_autocommit() {
        let transaction = c.unchecked_transaction().map_err(database_error)?;
        let result = commit_inner(&transaction, patch, context, payloads)?;
        transaction.commit().map_err(database_error)?;
        Ok(result)
    } else {
        commit_inner(c, patch, context, payloads)
    }
}

fn commit_inner(
    c: &Connection,
    patch: &StatePatch,
    context: &CommitContext<'_>,
    payloads: &BTreeMap<String, serde_json::Value>,
) -> Result<bool, String> {
    let mut ledger = load(c)?;
    // v2 payloads are content-addressed; verify before a re-send can be
    // accepted, so swapping the outer payload map is not silently accepted.
    super::world::validation_v2::verify_world_payload_refs(patch, payloads)?;
    // D17/C5: authorization and re-send detection before Source re-validation.
    match super::world::validation_v2::resend(&ledger, patch, context)? {
        super::world::validation_v2::Resend::Applied => return Ok(false),
        super::world::validation_v2::Resend::Conflict => {
            return Err("personal-patch-Conflict".into())
        }
        super::world::validation_v2::Resend::New => {}
    }
    for key in &context.input_dependencies {
        sources::revalidate(c, ledger.sources.get(key).ok_or("personal-source-missing")?)?;
    }
    // World semantics are validated here, before Ledger::apply, so direct
    // callers of store::commit cannot bypass them. Versioned validation covers
    // v1 and v2 and preserves the v1 semantic pass for v1-only patches.
    super::world::validation_v2::validate_commit_v2(c, &ledger, patch, context, payloads)?;
    if !ledger
        .apply(patch, context)
        .map_err(|e| format!("personal-patch-{e}"))?
    {
        return Ok(false);
    }
    for a in &patch.assertions {
        let payload = payloads
            .get(&a.payload_ref)
            .ok_or("personal-payload-missing")?;
        let text = encode(payload)?;
        if text.len() > 2000
            || context.issued_payload_bytes.get(&a.payload_ref) != Some(&text.len())
        {
            return Err("personal-payload-budget".into());
        }
        c.execute(
            "INSERT INTO personal_payloads VALUES(?1,?2,?3)",
            params![a.payload_ref, text, text.len()],
        )
        .map_err(database_error)?;
        c.execute(
            "INSERT INTO personal_assertions(id,metadata,payload_id) VALUES(?1,?2,?3)",
            params![a.id, encode(a)?, a.payload_ref],
        )
        .map_err(database_error)?;
        for key in &a.input_dependencies {
            dependency(c, &a.id, "source", &key.id)?;
        }
        for id in &a.depends_on {
            dependency(c, &a.id, "assertion", id)?;
        }
    }
    for t in &patch.transitions {
        c.execute(
            "INSERT INTO personal_transitions VALUES(?1,?2,?3,?4)",
            params![t.sequence, t.id, t.assertion_id, encode(t)?],
        )
        .map_err(database_error)?;
        for key in &t.input_dependencies {
            dependency(c, &t.assertion_id, "source", &key.id)?;
        }
    }
    for (key, status) in &patch.coverage {
        c.execute("INSERT INTO personal_coverage VALUES(?1,?2) ON CONFLICT(source_key) DO UPDATE SET status=excluded.status",params![encode(key)?,encode(status)?]).map_err(database_error)?;
    }
    c.execute(
        "INSERT INTO personal_patches VALUES(?1,?2)",
        params![patch.id, ledger.applied_patches[&patch.id]],
    )
    .map_err(database_error)?;
    let changed=c.execute("UPDATE personal_scope SET revision=?1 WHERE revision=?2 AND input_epoch=?3 AND policy_revision=?4",params![ledger.revision,patch.base_revision,patch.input_epoch,patch.policy_revision]).map_err(database_error)?;
    if changed != 1 {
        return Err("personal-patch-cas".into());
    }
    rebuild(c, context.now)?;
    Ok(true)
}
fn dependency(c: &Connection, a: &str, kind: &str, id: &str) -> Result<(), String> {
    c.execute(
        "INSERT OR IGNORE INTO personal_dependencies VALUES(?1,?2,?3)",
        params![a, kind, id],
    )
    .map_err(database_error)?;
    Ok(())
}
pub fn remember_source(c: &Connection, source: &SourceRef) -> Result<(), String> {
    sources::revalidate(c, source)?;
    let encoded = encode(source)?;
    let key = encode(&source.key)?;
    c.execute(
        "INSERT OR IGNORE INTO personal_source_refs VALUES(?1,?2)",
        params![key, encoded],
    )
    .map_err(database_error)?;
    let prior: String = c
        .query_row(
            "SELECT metadata FROM personal_source_refs WHERE key_json=?1",
            [key],
            |r| r.get(0),
        )
        .map_err(database_error)?;
    if prior != encoded {
        return Err("personal-source-conflict".into());
    }
    Ok(())
}
pub fn rebuild(c: &Connection, now: i64) -> Result<(), String> {
    let l = load(c)?;
    c.execute("DELETE FROM personal_projection", [])
        .map_err(database_error)?;
    for id in l.assertions.keys() {
        c.execute(
            "INSERT INTO personal_projection VALUES(?1,?2,?3,?4)",
            params![id, encode(&l.status(id, now))?, l.revision, now],
        )
        .map_err(database_error)?;
    }
    super::world::projection::rebuild(c, &l, now)?;
    Ok(())
}

/// Restore only after merging a verified copy of the CURRENT forget ledger.
/// None intentionally leaves the restored DB unavailable.
pub fn recover(c: &Connection, current: Option<&[(String, i64)]>) -> Result<(), String> {
    c.execute("UPDATE personal_scope SET recovery_ready=0", [])
        .map_err(database_error)?;
    let tombstones = current.ok_or("personal-recovery-tombstones-required")?;
    for (id, at) in tombstones {
        c.execute(
            "INSERT OR IGNORE INTO personal_tombstones VALUES(?1,?2)",
            params![id, at],
        )
        .map_err(database_error)?;
        c.execute("DELETE FROM conversation_messages WHERE id=?1", [id])
            .map_err(database_error)?;
    }
    c.execute(
        "UPDATE personal_generations SET output_allowed=0,status='interrupted'",
        [],
    )
    .map_err(database_error)?;
    c.execute("UPDATE personal_scope SET recovery_ready=1", [])
        .map_err(database_error)?;
    rebuild(c, super::now())
}
