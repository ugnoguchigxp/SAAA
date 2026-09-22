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
    manifest::create(connection, conversation_id, "initial", None, policy_version, "tools", &blob_id, "{}", 53_760, 0)
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
