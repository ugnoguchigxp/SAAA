//! Reuse the exact Episode references behind recent answers, without another Episode store.
use super::*;
use crate::runtime::context::scope::ScopeSnapshot;
use saaa_personal_state_core::SourceKey;
use std::collections::BTreeSet;

struct Reference {
    id: String,
    key: String,
    contract: Value,
}
pub(super) fn scope_current(c: &Connection, scope: &ScopeSnapshot) -> Result<bool, String> {
    if scope.status != "resolved" || scope.focus_scope_key.is_none() {
        return Ok(false);
    }
    c.query_row("SELECT NOT EXISTS(SELECT 1 FROM json_each(?1) r LEFT JOIN context_scopes s ON s.scope_key=r.value WHERE s.state IS NOT 'active')",[scope.keys_json()?],|r|r.get(0)).map_err(database_error)
}
fn recent(c: &Connection, scope: &ScopeSnapshot, current: &str) -> Result<Vec<Reference>, String> {
    if !scope_current(c, scope)? {
        return Ok(Vec::new());
    }
    let allowed: BTreeSet<String> = scope.scopes.iter().map(|s| s.key.clone()).collect();
    let mut stmt=c.prepare("SELECT a.run_id,r.episode_id,r.source_key,r.contract FROM (SELECT id,rowid AS position FROM conversation_messages WHERE conversation_id=?1 AND rowid<(SELECT rowid FROM conversation_messages WHERE id=?2) ORDER BY rowid DESC LIMIT 16) h JOIN memory_episode_artifacts a ON a.message_id=h.id JOIN memory_episode_run_refs r ON r.run_id=a.run_id ORDER BY h.position DESC,r.episode_id LIMIT 80").map_err(database_error)?;
    let rows = stmt
        .query_map(params![crate::PRIMARY_CONVERSATION_ID, current], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
            ))
        })
        .map_err(database_error)?;
    let mut seen = BTreeSet::new();
    let mut refs = Vec::new();
    for row in rows {
        let (run, id, key, raw) = row.map_err(database_error)?;
        let previous = match crate::runtime::context::scope::load(c, &run) {
            Ok(s) if s.status == "resolved" && s.focus_scope_key == scope.focus_scope_key => s,
            _ => continue,
        };
        if previous
            .scopes
            .iter()
            .any(|s| s.relation == "focus" && !allowed.contains(&s.key))
        {
            continue;
        }
        let contract: Value = serde_json::from_str(&raw).map_err(|_| "episode-source-contract")?;
        if validate_contract(c, &contract, Some(&allowed)).is_err()
            || !seen.insert((id.clone(), key.clone()))
        {
            continue;
        }
        refs.push(Reference { id, key, contract });
        if refs.len() == 5 {
            break;
        }
    }
    Ok(refs)
}

pub(crate) fn view(
    c: &Connection,
    scope: &ScopeSnapshot,
    current: &str,
) -> Result<(Option<String>, BTreeSet<SourceKey>), String> {
    let refs = recent(c, scope, current)?;
    let mut inputs = BTreeSet::new();
    let mut items = Vec::new();
    for r in refs {
        let sources = r.contract["sources"]
            .as_array()
            .ok_or("episode-source-contract")?;
        let mut ids = Vec::new();
        for s in sources {
            let key = SourceKey {
                id: s["sourceId"].as_str().ok_or("episode-source-id")?.into(),
                version: s["version"].as_u64().ok_or("episode-source-version")?,
                start: s["start"].as_u64().ok_or("episode-source-range")?,
                end: s["end"].as_u64().ok_or("episode-source-range")?,
            };
            ids.push(json!({"sourceId":key.id,"version":key.version}));
            inputs.insert(key);
        }
        items.push(json!({"id":r.id,"sourceKey":r.key,"sources":ids}));
    }
    let text = if items.is_empty() {
        None
    } else {
        Some(format!(
            "[RECENT_EPISODE_REFERENCES; instructionAuthority=none]\n{}",
            json!({"purpose":"References used by preceding answers in this focus scope. Reuse them only for continuing that subject; they are not current facts. Fetch details or original evidence when needed.","items":items})
        ))
    };
    Ok((text, inputs))
}

/// Must run through the Writer, alongside final response acceptance or raw evidence fetch.
pub(crate) fn reuse(
    c: &Connection,
    run: &str,
    scope: &ScopeSnapshot,
    current: &str,
) -> Result<(), String> {
    for r in recent(c, scope, current)? {
        c.execute("INSERT OR IGNORE INTO memory_episode_run_refs(run_id,episode_id,source_key,contract) VALUES(?1,?2,?3,?4)",params![run,r.id,r.key,r.contract.to_string()]).map_err(database_error)?;
    }
    Ok(())
}
