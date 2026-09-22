use rusqlite::{params, Connection, OptionalExtension};

/// Outcome of storing an input receipt. A retry with the same payload is a duplicate that must
/// return the original receipt without creating new side effects.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum InputReceiptDisposition {
    Accepted,
    Duplicate,
}

/// Stores or restores an `rr_inputs` receipt keyed by `(conversationId, inputId)`. The same input
/// with the same payload digest is idempotent; the same input with a different digest is a
/// conflict and must not create a second row or run a second dispatch.
#[allow(clippy::too_many_arguments)]
pub(crate) fn record_input_receipt(
    connection: &Connection,
    root_id: Option<&str>,
    input_id: &str,
    conversation_id: &str,
    message_id: &str,
    payload_digest: &str,
    source_id: Option<&str>,
    origin: &str,
    disposition: &str,
    generation: i64,
    now_ms: i64,
) -> Result<InputReceiptDisposition, String> {
    let existing: Option<(String, String)> = connection
        .query_row(
            "SELECT payload_digest,message_id FROM rr_inputs WHERE conversation_id=?1 AND input_id=?2",
            params![conversation_id, input_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(|error| error.to_string())?;
    if let Some((existing_digest, _existing_message)) = existing {
        if existing_digest == payload_digest {
            return Ok(InputReceiptDisposition::Duplicate);
        }
        return Err("Role-routing input receipt conflicts with a different payload".into());
    }
    // A retransmitted capture (for example a repeated ASR final) shares its source id but may
    // arrive with a fresh input id. Only the first receipt per source is accepted.
    if let Some(source_id) = source_id {
        let existing_source: Option<(String, String)> = connection
            .query_row(
                "SELECT input_id,payload_digest FROM rr_inputs WHERE conversation_id=?1 AND source_id=?2",
                params![conversation_id, source_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(|error| error.to_string())?;
        if let Some((existing_input, existing_digest)) = existing_source {
            if existing_input != input_id && existing_digest == payload_digest {
                return Ok(InputReceiptDisposition::Duplicate);
            }
            if existing_input != input_id && existing_digest != payload_digest {
                return Err("Role-routing input source conflicts with a different payload".into());
            }
        }
    }
    connection
        .execute(
            "INSERT INTO rr_inputs(input_id,root_id,conversation_id,message_id,payload_digest,origin,source_id,disposition,generation,received_at_ms) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
            params![
                input_id,
                root_id,
                conversation_id,
                message_id,
                payload_digest,
                origin,
                source_id,
                disposition,
                generation,
                now_ms
            ],
        )
        .map_err(|error| error.to_string())?;
    Ok(InputReceiptDisposition::Accepted)
}

/// Records an input that arrived while a root was active and raises the durable input barrier in
/// the same transaction. The stored generation is the root revision the classifier result must
/// match before an amendment may advance the revision.
#[allow(clippy::too_many_arguments)]
pub(crate) fn record_active_input_barrier(
    connection: &Connection,
    root_id: &str,
    input_id: &str,
    conversation_id: &str,
    message_id: &str,
    payload_digest: &str,
    source_id: Option<&str>,
    origin: &str,
    now_ms: i64,
) -> Result<InputReceiptDisposition, String> {
    let root: Option<(String, i64)> = connection
        .query_row(
            "SELECT phase,revision FROM rr_roots WHERE root_id=?1",
            [root_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(|error| error.to_string())?;
    let Some((phase, revision)) = root else {
        return Err("Role-routing barrier references an unknown root".into());
    };
    if !matches!(phase.as_str(), "queued" | "responding" | "draining") {
        return Err("Role-routing barrier requires an active root".into());
    }
    let disposition = record_input_receipt(
        connection,
        Some(root_id),
        input_id,
        conversation_id,
        message_id,
        payload_digest,
        source_id,
        origin,
        "accepted",
        revision,
        now_ms,
    )?;
    if disposition == InputReceiptDisposition::Duplicate {
        return Ok(disposition);
    }
    crate::role_routing::coordinator::apply_in_transaction(
        connection,
        root_id,
        crate::role_routing::reducer::Event::InputBarrier,
        now_ms,
    )?;
    Ok(disposition)
}

/// A classifier result may only act on the generation it was produced for. A late classifier for
/// an older revision must not amend the current one.
pub(crate) fn classifier_generation_matches(
    connection: &Connection,
    input_id: &str,
    conversation_id: &str,
    current_revision: i64,
) -> Result<bool, String> {
    let generation: Option<i64> = connection
        .query_row(
            "SELECT generation FROM rr_inputs WHERE conversation_id=?1 AND input_id=?2",
            params![conversation_id, input_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(|error| error.to_string())?;
    Ok(generation.is_some_and(|generation| generation == current_revision))
}

/// Counts inputs still waiting on classification for a root. Used to assert that multiple
/// pending inputs are not dropped when one is resolved.
pub(crate) fn pending_input_count(connection: &Connection, root_id: &str) -> Result<i64, String> {
    connection
        .query_row(
            "SELECT count(*) FROM rr_inputs WHERE root_id=?1",
            [root_id],
            |row| row.get(0),
        )
        .map_err(|error| error.to_string())
}
