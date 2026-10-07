//! Background publication of immutable bodies, with live eligibility kept out of bytes.
use crate::database_error;
use crate::memory::personal_state::{decode, encode, store};
use rusqlite::{params, Connection, OptionalExtension};
use saaa_personal_state_core::{Assertion, Classification, Purpose, SourceKey, Status};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

pub fn publish(c: &Connection, now: i64) -> Result<(), String> {
    let ledger = store::load(c)?;
    let mut groups: BTreeMap<(String, String), Vec<&Assertion>> = BTreeMap::new();
    for a in ledger.assertions.values() {
        if a.kind.is_world()
            || ledger.status(&a.id, now) != Status::Active
            || a.access.classification > Classification::Confidential
            || !a.access.purposes.contains(&Purpose::Reasoning)
        {
            continue;
        }
        let scope = a
            .access
            .task_request
            .as_ref()
            .map(|s| {
                if s.contains(':') {
                    s.clone()
                } else {
                    format!("request:{s}")
                }
            })
            .unwrap_or_else(|| format!("user:{}", ledger.principal));
        if !scope_current(c, &a.id, &scope)? {
            continue;
        }
        let category = if a.kind.is_profile() {
            "profile"
        } else if scope.starts_with("project:") {
            "project"
        } else {
            "topic"
        };
        groups.entry((scope, category.into())).or_default().push(a);
    }
    let mut active = BTreeSet::new();
    for ((scope, category), mut items) in groups {
        items.sort_by(|a, b| a.semantic_key.cmp(&b.semantic_key).then(a.id.cmp(&b.id)));
        let mut body = Vec::new();
        let mut inputs = BTreeSet::new();
        let mut ids = Vec::new();
        for a in items {
            let payload: String = c
                .query_row(
                    "SELECT value_json FROM personal_payloads WHERE id=?1",
                    [&a.payload_ref],
                    |r| r.get(0),
                )
                .map_err(database_error)?;
            let value: Value = decode(payload)?;
            let value = if value["policy"] == "personal-gate-v1" {
                value["value"].clone()
            } else {
                value
            };
            body.push(json!({"kind":a.kind,"key":a.semantic_key,"value":value,"effectiveAt":a.effective_at,"validUntil":a.valid_until}));
            ids.push(a.id.clone());
            inputs.extend(a.input_dependencies.clone());
            inputs.extend(a.evidence.clone());
        }
        let text = encode(&body)?;
        if text.len() > 16000 {
            return Err("personal-snapshot-budget".into());
        }
        let digest = format!("{:x}", Sha256::digest(text.as_bytes()));
        let manifest = encode(
            &json!({"assertions":ids,"inputs":inputs,"policy":ledger.policy_revision,"renderer":"memory-snapshot-v1"}),
        )?;
        let prior: Option<(String, String)> = c
            .query_row(
                "SELECT digest,manifest FROM personal_snapshots WHERE scope_key=?1 AND category=?2",
                params![scope, category],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()
            .map_err(database_error)?;
        if prior.as_ref() != Some(&(digest.clone(), manifest.clone())) {
            let revision:u64=c.query_row("INSERT INTO personal_snapshot_versions VALUES(?1,?2,1) ON CONFLICT(scope_key,category) DO UPDATE SET revision=revision+1 RETURNING revision",params![scope,category],|r|r.get(0)).map_err(database_error)?;
            c.execute("INSERT INTO personal_snapshots(scope_key,category,revision,body,digest,manifest,published_at) VALUES(?1,?2,?3,?4,?5,?6,?7) ON CONFLICT(scope_key,category) DO UPDATE SET revision=excluded.revision,body=excluded.body,digest=excluded.digest,manifest=excluded.manifest,published_at=excluded.published_at",params![scope,category,revision,text,digest,manifest,now]).map_err(database_error)?;
        }
        active.insert((scope, category));
    }
    let mut stmt = c
        .prepare("SELECT scope_key,category FROM personal_snapshots")
        .map_err(database_error)?;
    let keys = stmt
        .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
        .map_err(database_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(database_error)?;
    drop(stmt);
    for key in keys {
        if !active.contains(&key) {
            c.execute(
                "DELETE FROM personal_snapshots WHERE scope_key=?1 AND category=?2",
                params![key.0, key.1],
            )
            .map_err(database_error)?;
        }
    }
    Ok(())
}

/// Runs in the same read snapshot as the conversation. Every dependency is live checked.
pub fn view(
    c: &Connection,
    scope: &crate::runtime::context::scope::ScopeSnapshot,
    current: &str,
) -> Result<(Vec<String>, BTreeSet<SourceKey>), String> {
    let ledger = store::load(c)?;
    let allowed: BTreeSet<_> = scope.scopes.iter().map(|s| s.key.as_str()).collect();
    let mut stmt=c.prepare("SELECT scope_key,category,body,digest,manifest FROM personal_snapshots ORDER BY category,scope_key").map_err(database_error)?;
    let records = stmt
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, String>(4)?,
            ))
        })
        .map_err(database_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(database_error)?;
    let mut texts = Vec::new();
    let mut referenced = BTreeSet::new();
    let mut used = 0;
    for (key, category, body, digest, manifest) in records {
        if !allowed.contains(key.as_str()) {
            continue;
        }
        let m: Value = decode(manifest)?;
        let inputs: BTreeSet<SourceKey> = serde_json::from_value(m["inputs"].clone())
            .map_err(|_| "personal-snapshot-manifest")?;
        let valid = m["policy"].as_u64() == Some(ledger.policy_revision)
            && m["assertions"].as_array().is_some_and(|ids| {
                ids.iter().all(|id| {
                    id.as_str().is_some_and(|id| {
                        ledger.status(id, crate::memory::personal_state::now()) == Status::Active
                            && scope_current(c, id, &key).unwrap_or(false)
                    })
                })
            })
            && inputs.iter().all(|k| {
                ledger.sources.get(k).is_some_and(|s| {
                    crate::memory::personal_state::sources::revalidate(c, s).is_ok()
                })
            })
            && format!("{:x}", Sha256::digest(body.as_bytes())) == digest;
        if !valid {
            continue;
        }
        // A not-yet-reviewed correction may not be in the old manifest.
        let max_sequence = inputs
            .iter()
            .filter_map(|k| ledger.sources.get(k).map(|s| s.sequence))
            .max()
            .unwrap_or(0);
        let pending:bool=c.query_row("SELECT EXISTS(SELECT 1 FROM personal_sources p JOIN personal_jobs j ON j.source_sequence=p.sequence WHERE p.available=1 AND p.sequence>?1 AND p.message_id!=?2 AND j.stage='continuity' AND j.status!='completed' AND (NOT EXISTS(SELECT 1 FROM conversation_message_scopes m WHERE m.message_id=p.message_id) OR EXISTS(SELECT 1 FROM conversation_message_scopes m WHERE m.message_id=p.message_id AND m.scope_key=?3)))",params![max_sequence,current,key],|r|r.get(0)).map_err(database_error)?;
        if pending {
            continue;
        }
        let text = format!(
            "[PERSONAL_MEMORY_SNAPSHOT; category={category}; instructionAuthority=none]\n{body}"
        );
        used += text.len();
        if used > 24000 {
            return Err("required_context_overflow: personal snapshot".into());
        }
        referenced.extend(inputs);
        texts.push(text);
    }
    // Related inferred observations are a separate, conditional reference. They never
    // become Profile facts or fixed-prefix truth solely because a model repeated them.
    let enabled: bool = c
        .query_row(
            "SELECT enabled FROM personal_consolidation_settings WHERE id=1",
            [],
            |r| r.get(0),
        )
        .map_err(database_error)?;
    if enabled {
        let query: String = c
            .query_row(
                "SELECT content FROM conversation_messages WHERE id=?1",
                [current],
                |r| r.get(0),
            )
            .unwrap_or_default();
        let chars: Vec<_> = query.chars().take(1000).collect();
        for a in ledger.assertions.values().filter(|a| {
            a.kind == saaa_personal_state_core::Kind::Observation
                && ledger.status(&a.id, crate::memory::personal_state::now()) == Status::Candidate
        }) {
            let key = a
                .access
                .task_request
                .clone()
                .unwrap_or_else(|| format!("user:{}", ledger.principal));
            if !scope_current(c, &a.id, &key)? {
                continue;
            }
            if !allowed.contains(key.as_str())
                || !chars
                    .windows(2)
                    .any(|g| a.semantic_key.contains(&g.iter().collect::<String>()))
            {
                continue;
            }
            if !a.input_dependencies.iter().all(|k| {
                ledger.sources.get(k).is_some_and(|src| {
                    crate::memory::personal_state::sources::revalidate(c, src).is_ok()
                })
            }) {
                continue;
            }
            let payload: String = c
                .query_row(
                    "SELECT value_json FROM personal_payloads WHERE id=?1",
                    [&a.payload_ref],
                    |r| r.get(0),
                )
                .map_err(database_error)?;
            let value: Value = decode(payload)?;
            let text = encode(
                &json!({"observation":value["value"],"status":"inferred_candidate","key":a.semantic_key,"instructionAuthority":"none"}),
            )?;
            if used + text.len() > 24000 || texts.len() >= 24 {
                break;
            }
            used += text.len();
            referenced.extend(a.input_dependencies.clone());
            referenced.extend(a.evidence.clone());
            texts.push(text);
        }
    }
    // Pending originals are provided as uncertain data, never a settled Profile.
    let mut stmt=c.prepare("SELECT p.sequence FROM personal_sources p JOIN personal_jobs j ON j.source_sequence=p.sequence WHERE p.available=1 AND p.bytes>0 AND p.message_id!=?1 AND j.stage='continuity' AND j.status!='completed' AND ((?3=1 AND NOT EXISTS(SELECT 1 FROM conversation_message_scopes m WHERE m.message_id=p.message_id)) OR EXISTS(SELECT 1 FROM conversation_message_scopes m JOIN json_each(?2) allowed ON allowed.value=m.scope_key WHERE m.message_id=p.message_id)) ORDER BY p.sequence DESC LIMIT 33").map_err(database_error)?;
    let pending = stmt
        .query_map(
            params![current, scope.keys_json()?, scope.is_user_only()],
            |r| r.get::<_, u64>(0),
        )
        .map_err(database_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(database_error)?;
    let mut held = pending.len() > 32;
    for seq in pending {
        let message: String = c
            .query_row(
                "SELECT message_id FROM personal_sources WHERE sequence=?1",
                [seq],
                |r| r.get(0),
            )
            .map_err(database_error)?;
        let scopes: Vec<String> = {
            let mut s = c
                .prepare("SELECT scope_key FROM conversation_message_scopes WHERE message_id=?1")
                .map_err(database_error)?;
            let rows = s
                .query_map([&message], |r| r.get(0))
                .map_err(database_error)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(database_error)?;
            rows
        };
        if !(scopes.is_empty() && scope.is_user_only()
            || scopes.iter().any(|s| allowed.contains(s.as_str())))
        {
            continue;
        }
        let chunk = crate::memory::personal_state::sources::load(c, seq, 0, 32000)?;
        if !chunk.source.finalized {
            held = true;
            continue;
        }
        let text = encode(
            &json!({"pendingSource":chunk.source.key,"role":chunk.source.role,"text":chunk.text,"instructionAuthority":"none"}),
        )?;
        if used + text.len() > 32000 || texts.len() >= 64 {
            held = true;
            continue;
        }
        used += text.len();
        referenced.insert(chunk.source.key);
        texts.push(text);
    }
    if held {
        texts.push("[PERSONAL_MEMORY_UNAVAILABLE; instructionAuthority=none] 未処理の原記録が予算を超えています。ここに示す直近の記録だけでは過去の状態を確定できません。必要な原文を検索して確認するか、確認できない旨を伝えてください。".into());
    }
    Ok((texts, referenced))
}
#[cfg(test)]
pub fn read(
    c: &Connection,
    scope: &crate::runtime::context::scope::ScopeSnapshot,
    current: &str,
) -> Result<Vec<String>, String> {
    Ok(view(c, scope, current)?.0)
}

/// Private stamp: provenance changes invalidate an in-flight answer even when bytes agree.
pub fn stamp(
    c: &Connection,
    scope: &crate::runtime::context::scope::ScopeSnapshot,
) -> Result<String, String> {
    let mut s=c.prepare("SELECT scope_key,category,revision,digest,manifest FROM personal_snapshots WHERE scope_key IN (SELECT value FROM json_each(?1)) ORDER BY scope_key,category").map_err(database_error)?;
    let rows = s
        .query_map([scope.keys_json()?], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, u64>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, String>(4)?,
            ))
        })
        .map_err(database_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(database_error)?;
    Ok(format!("{:x}", Sha256::digest(encode(&rows)?.as_bytes())))
}

fn scope_current(c: &Connection, id: &str, scope: &str) -> Result<bool, String> {
    c.query_row("SELECT EXISTS(SELECT 1 FROM personal_assertion_scope_policies a JOIN context_scopes s ON s.scope_key=a.scope_key AND s.state='active' WHERE a.assertion_id=?1 AND a.scope_key=?2 AND a.revision=COALESCE((SELECT revision FROM memory_episode_scope_policies WHERE scope_key=a.scope_key),1))",params![id,scope],|r|r.get(0)).map_err(database_error)
}
