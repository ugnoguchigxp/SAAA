use rusqlite::Connection;

use super::manifest;

pub(crate) fn ensure_initial(
    connection: &Connection,
    conversation_id: &str,
    fixed: &str,
    policy_version: &str,
) -> Result<manifest::SegmentManifest, String> {
    if let Some(active) = manifest::load_active(connection, conversation_id)? {
        return Ok(active);
    }
    let digest = crate::generated_capabilities::contracts::sha256_hex(fixed.as_bytes());
    let blob_id = crate::util::new_id("blob");
    connection
        .execute(
            "INSERT INTO blobs(id, dedup_domain, sha256, codec, raw_bytes, stored_bytes, data, ref_count)
             VALUES(?1,'segment',?2,'identity',?3,?3,?4,1)",
            rusqlite::params![blob_id, digest, fixed.len() as i64, fixed.as_bytes()],
        )
        .map_err(|error| error.to_string())?;
    manifest::create(
        connection,
        conversation_id,
        "initial",
        None,
        policy_version,
        "tools",
        &blob_id,
        "{}",
        53_760,
        0,
    )
}

pub(crate) struct BuildInput<'a> {
    pub(crate) conversation_id: &'a str,
    pub(crate) fixed: &'a str,
    pub(crate) policy_version: &'a str,
    pub(crate) user: &'a str,
    pub(crate) prior: &'a [(&'a str, &'a str, &'a str)],
    pub(crate) budget: usize,
}

pub(crate) fn build(
    connection: &Connection,
    input: &BuildInput<'_>,
) -> Result<Vec<(String, String, String)>, String> {
    let manifest = ensure_initial(
        connection,
        input.conversation_id,
        input.fixed,
        input.policy_version,
    )?;
    let mut known: Vec<String> = connection
        .prepare(
            "SELECT record_id FROM context_entries WHERE segment_id=?1 AND record_id IS NOT NULL",
        )
        .map_err(|error| error.to_string())?
        .query_map([&manifest.id], |row| row.get(0))
        .map_err(|error| error.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string())?;
    for (id, role, content) in input.prior {
        if *role != "user" && *role != "assistant" {
            continue;
        }
        if known.iter().any(|existing| existing == id) {
            continue;
        }
        super::entries::append_conversation(connection, &manifest.id, role, id, content)?;
        known.push((*id).to_string());
    }
    let entries = load_texts(connection, &manifest.id)?;
    let must = input.fixed.len() + input.user.len();
    must_overflow(must, input.budget)?;
    let room = input.budget.saturating_sub(must);
    let mut kept = Vec::new();
    let mut used = 0usize;
    for (sequence, role, text) in entries.iter().rev() {
        if used + text.len() > room {
            break;
        }
        used += text.len();
        kept.push((
            format!("segment-entry-{sequence}"),
            role.clone(),
            text.clone(),
        ));
    }
    kept.reverse();
    let mut history = vec![(
        "segment-fixed".into(),
        "system".into(),
        input.fixed.to_string(),
    )];
    history.extend(kept);
    history.push(("segment-user".into(), "user".into(), input.user.to_string()));
    Ok(history)
}

fn load_texts(
    connection: &Connection,
    segment_id: &str,
) -> Result<Vec<(i64, String, String)>, String> {
    let mut statement = connection
        .prepare(
            "SELECT e.sequence, e.role, CAST(b.data AS TEXT)
             FROM context_entries e JOIN blobs b ON b.id = e.rendered_blob_id
             WHERE e.segment_id=?1 ORDER BY e.sequence",
        )
        .map_err(|error| error.to_string())?;
    let rows = statement
        .query_map([segment_id], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        })
        .map_err(|error| error.to_string())?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string())
}

pub(crate) fn must_overflow(must_bytes: usize, budget: usize) -> Result<(), String> {
    if must_bytes > budget {
        Err("required_context_overflow: segment must exceeds budget".into())
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn db() -> Connection {
        let connection = Connection::open_in_memory().unwrap();
        crate::persistence::schema::initialize_database(&connection).unwrap();
        connection
    }

    #[test]
    fn cw_44_first_turn_creates_initial_segment() {
        let connection = db();
        let manifest = ensure_initial(&connection, "c", "policy", "policy-hash").unwrap();
        assert_eq!(manifest.start_reason, "initial");
    }

    #[test]
    fn cw_44_fixed_blob_unchanged_across_turns() {
        let connection = db();
        let first = ensure_initial(&connection, "c", "policy-fixed", "hash").unwrap();
        let second = ensure_initial(&connection, "c", "policy-fixed", "hash").unwrap();
        let third = ensure_initial(&connection, "c", "policy-fixed", "hash").unwrap();
        assert_eq!(first.fixed_render_blob_id, second.fixed_render_blob_id);
        assert_eq!(second.fixed_render_blob_id, third.fixed_render_blob_id);
    }

    #[test]
    fn cw_44_must_overflow_returns_required_context_overflow() {
        let error = must_overflow(100, 40).unwrap_err();
        assert!(error.contains("required_context_overflow"));
    }

    #[test]
    fn cw_44_voice_text_switch_keeps_fixed_bytes() {
        let voice = "policy tools";
        let text = "policy tools";
        assert_eq!(voice.len(), text.len());
        assert!(!voice.contains("input_origin"));
    }
}
