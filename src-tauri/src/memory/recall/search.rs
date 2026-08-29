use std::collections::{HashMap, HashSet};

use chrono_tz::Tz;
use rusqlite::{params, params_from_iter, types::Value as SqlValue, Connection};
use sha2::{Digest, Sha256};

use super::super::contracts::{
    RecallConversationOutput, RecallError, RecallErrorCode, RecallEvent, RecallWindow,
    ResolvedTimeRange,
};
use super::*;

pub(super) fn search_candidates(
    connection: &Connection,
    query: Option<&str>,
    range: Option<&ResolvedRange>,
    current_message_id: &str,
    snapshot_max_rowid: i64,
    offset: usize,
    limit: usize,
) -> Result<Vec<Candidate>, RecallError> {
    let terms = query
        .map(|value| {
            value
                .split_whitespace()
                .take(MAX_QUERY_TERMS)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let long_terms = terms
        .iter()
        .copied()
        .filter(|term| term.chars().count() >= 3)
        .collect::<Vec<_>>();
    let use_fts = !long_terms.is_empty();
    let mut sql = if use_fts {
        "SELECT m.id,m.conversation_id,bm25(conversation_messages_fts) AS relevance
         FROM conversation_messages_fts
         JOIN conversation_messages m ON m.id=conversation_messages_fts.message_id
         WHERE conversation_messages_fts.content MATCH ?"
            .to_string()
    } else {
        "SELECT m.id,m.conversation_id,0.0 AS relevance
         FROM conversation_messages m
         WHERE m.role IN ('user','assistant','transcript')"
            .to_string()
    };
    let mut values = Vec::<SqlValue>::new();
    if use_fts {
        values.push(SqlValue::Text(
            long_terms
                .iter()
                .map(|term| format!("\"{}\"", term.replace('"', "\"\"")))
                .collect::<Vec<_>>()
                .join(" AND "),
        ));
    }
    sql.push_str(" AND m.id != ?");
    values.push(SqlValue::Text(current_message_id.to_string()));
    sql.push_str(" AND m.rowid <= ?");
    values.push(SqlValue::Integer(snapshot_max_rowid));
    for term in terms {
        if use_fts && term.chars().count() >= 3 {
            continue;
        }
        sql.push_str(" AND m.content LIKE ? ESCAPE '\\' COLLATE NOCASE");
        values.push(SqlValue::Text(format!("%{}%", escape_like(term))));
    }
    if let Some(range) = range {
        sql.push_str(
            " AND CAST(m.created_at AS INTEGER) >= ?
              AND CAST(m.created_at AS INTEGER) < ?",
        );
        values.push(SqlValue::Integer(range.from_ms));
        values.push(SqlValue::Integer(range.to_exclusive_ms));
    }
    if use_fts {
        sql.push_str(" ORDER BY relevance ASC, CAST(m.created_at AS INTEGER) DESC, m.rowid DESC");
    } else {
        sql.push_str(" ORDER BY CAST(m.created_at AS INTEGER) DESC, m.rowid DESC");
    }
    sql.push_str(" LIMIT ? OFFSET ?");
    values.push(SqlValue::Integer(
        i64::try_from(limit).map_err(|_| local_unavailable(""))?,
    ));
    values.push(SqlValue::Integer(
        i64::try_from(offset).map_err(|_| local_unavailable(""))?,
    ));
    let mut statement = connection.prepare(&sql).map_err(local_unavailable)?;
    let candidates = statement
        .query_map(params_from_iter(values), |row| {
            Ok(Candidate {
                id: row.get(0)?,
                conversation_id: row.get(1)?,
                score: row.get(2)?,
            })
        })
        .map_err(local_unavailable)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(local_unavailable)?;
    Ok(candidates)
}

pub(super) fn load_window(
    connection: &Connection,
    candidate: &Candidate,
    range: Option<&ResolvedRange>,
    current_message_id: &str,
    snapshot_max_rowid: i64,
) -> Result<InternalWindow, RecallError> {
    let (anchor_turn, anchor_event_sequence): (i64, i64) = connection
        .query_row(
            "WITH ordered AS (
               SELECT id,
                 ROW_NUMBER() OVER (
                   ORDER BY CAST(created_at AS INTEGER),rowid
                 ) AS event_sequence,
                 SUM(CASE WHEN role='user' THEN 1 ELSE 0 END) OVER (
                   ORDER BY CAST(created_at AS INTEGER),rowid
                 ) AS turn_sequence
               FROM conversation_messages
               WHERE conversation_id=?1 AND id != ?3 AND rowid <= ?4
                 AND role IN ('user','assistant','transcript')
             )
             SELECT turn_sequence,event_sequence FROM ordered WHERE id=?2",
            params![
                candidate.conversation_id,
                candidate.id,
                current_message_id,
                snapshot_max_rowid
            ],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(local_unavailable)?;
    let first_turn = anchor_turn.saturating_sub(MAX_NEIGHBOR_TURNS);
    let last_turn = anchor_turn.saturating_add(MAX_NEIGHBOR_TURNS);
    let mut statement = connection
        .prepare(
            "WITH ordered AS (
               SELECT rowid,id,conversation_id,role,content,CAST(created_at AS INTEGER) AS created_at_ms,
                 ROW_NUMBER() OVER (
                   ORDER BY CAST(created_at AS INTEGER),rowid
                 ) AS event_sequence,
                 SUM(CASE WHEN role='user' THEN 1 ELSE 0 END) OVER (
                   ORDER BY CAST(created_at AS INTEGER),rowid
                 ) AS turn_sequence
               FROM conversation_messages
               WHERE conversation_id=?1 AND id != ?4 AND rowid <= ?5
                 AND role IN ('user','assistant','transcript')
             ), nearby AS (
               SELECT rowid,id,conversation_id,role,content,created_at_ms,turn_sequence,event_sequence
               FROM ordered
               WHERE turn_sequence BETWEEN ?2 AND ?3
               ORDER BY ABS(event_sequence - ?6),event_sequence
               LIMIT ?7
             )
             SELECT rowid,id,conversation_id,role,content,created_at_ms,turn_sequence
             FROM nearby
             ORDER BY created_at_ms,rowid",
        )
        .map_err(local_unavailable)?;
    let events = statement
        .query_map(
            params![
                candidate.conversation_id,
                first_turn,
                last_turn,
                current_message_id,
                snapshot_max_rowid,
                anchor_event_sequence,
                i64::try_from(MAX_EVENTS_PER_WINDOW).map_err(|_| local_unavailable(""))?
            ],
            |row| {
                Ok(InternalEvent {
                    rowid: row.get(0)?,
                    id: row.get(1)?,
                    conversation_id: row.get(2)?,
                    role: row.get(3)?,
                    content: row.get(4)?,
                    created_at_ms: row.get(5)?,
                    turn_sequence: row.get(6)?,
                })
            },
        )
        .map_err(local_unavailable)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(local_unavailable)?;
    if events
        .iter()
        .any(|event| DateTime::<Utc>::from_timestamp_millis(event.created_at_ms).is_none())
    {
        return Err(local_unavailable(""));
    }
    let events = events
        .into_iter()
        .filter(|event| {
            event.turn_sequence == anchor_turn
                || range.is_none_or(|value| {
                    event.created_at_ms >= value.from_ms
                        && event.created_at_ms < value.to_exclusive_ms
                })
        })
        .collect::<Vec<_>>();
    let actual_first = events
        .iter()
        .map(|event| event.turn_sequence)
        .min()
        .unwrap_or(anchor_turn);
    let actual_last = events
        .iter()
        .map(|event| event.turn_sequence)
        .max()
        .unwrap_or(anchor_turn);
    Ok(InternalWindow {
        conversation_id: candidate.conversation_id.clone(),
        first_turn: actual_first,
        last_turn: actual_last,
        score: score(candidate.score),
        matched_event_refs: vec![candidate.id.clone()],
        events,
    })
}

pub(super) fn windows_overlap(left: &InternalWindow, right: &InternalWindow) -> bool {
    left.conversation_id == right.conversation_id
        && left.first_turn <= right.last_turn.saturating_add(1)
        && right.first_turn <= left.last_turn.saturating_add(1)
}

pub(super) fn merge_windows(target: &mut InternalWindow, source: InternalWindow) {
    target.first_turn = target.first_turn.min(source.first_turn);
    target.last_turn = target.last_turn.max(source.last_turn);
    target.score = target.score.max(source.score);
    for event_ref in source.matched_event_refs {
        if !target.matched_event_refs.contains(&event_ref) {
            target.matched_event_refs.push(event_ref);
        }
    }
    let mut known = target
        .events
        .iter()
        .map(|event| event.id.clone())
        .collect::<HashSet<_>>();
    for event in source.events {
        if known.insert(event.id.clone()) {
            target.events.push(event);
        }
    }
    if target.events.len() > MAX_MERGED_EVENTS_PER_WINDOW {
        let matched = target
            .matched_event_refs
            .iter()
            .cloned()
            .collect::<HashSet<_>>();
        let matched_turns = target
            .events
            .iter()
            .filter(|event| matched.contains(&event.id))
            .map(|event| event.turn_sequence)
            .collect::<HashSet<_>>();
        target.events.sort_by_key(|event| {
            let distance = matched_turns
                .iter()
                .map(|turn| (event.turn_sequence - *turn).unsigned_abs())
                .min()
                .unwrap_or(u64::MAX);
            (
                !matched.contains(&event.id),
                distance,
                event.created_at_ms,
                event.rowid,
            )
        });
        target.events.truncate(MAX_MERGED_EVENTS_PER_WINDOW);
    }
    target
        .events
        .sort_by_key(|event| (event.created_at_ms, event.rowid));
}

pub(super) fn project_windows(
    windows: Vec<InternalWindow>,
    timezone: Tz,
) -> (Vec<RecallWindow>, bool) {
    if windows.is_empty() {
        return (Vec::new(), false);
    }
    let allowance = MAX_OUTPUT_TOKEN_BUDGET / windows.len();
    let mut any_truncated = false;
    let projected = windows
        .into_iter()
        .filter_map(|window| {
            let matched = window
                .matched_event_refs
                .iter()
                .cloned()
                .collect::<HashSet<_>>();
            let matched_turns = window
                .events
                .iter()
                .filter(|event| matched.contains(&event.id))
                .map(|event| event.turn_sequence)
                .collect::<HashSet<_>>();
            let mut matched_events = window
                .events
                .iter()
                .filter(|event| matched.contains(&event.id))
                .cloned()
                .collect::<Vec<_>>();
            matched_events.sort_by_key(|event| (event.created_at_ms, event.rowid));
            let mut neighbors = window
                .events
                .iter()
                .filter(|event| !matched.contains(&event.id))
                .cloned()
                .collect::<Vec<_>>();
            neighbors.sort_by_key(|event| {
                let distance = matched_turns
                    .iter()
                    .map(|turn| (event.turn_sequence - *turn).unsigned_abs())
                    .min()
                    .unwrap_or(u64::MAX);
                (distance, event.created_at_ms, event.rowid)
            });
            let mut remaining = allowance;
            let mut selected = HashMap::<String, RecallEvent>::new();
            let matched_count = matched_events.len();
            for (index, event) in matched_events.into_iter().enumerate() {
                let events_left = matched_count.saturating_sub(index).max(1);
                let event_allowance = remaining / events_left;
                let event_id = event.id.clone();
                match project_event(event, event_allowance, timezone) {
                    Some((projected, used)) => {
                        any_truncated |= projected.truncated;
                        remaining = remaining.saturating_sub(used);
                        selected.insert(event_id, projected);
                    }
                    None => any_truncated = true,
                }
            }
            for event in neighbors {
                let event_id = event.id.clone();
                match project_event(event, remaining, timezone) {
                    Some((projected, used)) => {
                        any_truncated |= projected.truncated;
                        remaining = remaining.saturating_sub(used);
                        selected.insert(event_id, projected);
                    }
                    None => {
                        any_truncated = true;
                        break;
                    }
                }
            }
            let selected_ids = selected.keys().cloned().collect::<HashSet<_>>();
            let mut events = window
                .events
                .iter()
                .filter_map(|event| selected.remove(&event.id))
                .collect::<Vec<_>>();
            if events.is_empty() {
                return None;
            }
            let start_event_ref = events.first()?.event_ref.clone();
            let end_event_ref = events.last()?.event_ref.clone();
            Some(RecallWindow {
                window_ref: opaque_ref("window", &format!("{start_event_ref}:{end_event_ref}")),
                score: window.score,
                matched_event_refs: window
                    .matched_event_refs
                    .iter()
                    .filter(|event_id| selected_ids.contains(*event_id))
                    .map(|event_id| opaque_ref("event", event_id))
                    .collect(),
                start_event_ref,
                end_event_ref,
                events: std::mem::take(&mut events),
            })
        })
        .collect::<Vec<_>>();
    (projected, any_truncated)
}

pub(super) fn project_event(
    event: InternalEvent,
    allowance: usize,
    timezone: Tz,
) -> Option<(RecallEvent, usize)> {
    if event.content.is_empty() {
        return Some((
            RecallEvent {
                event_ref: opaque_ref("event", &event.id),
                turn_ref: opaque_ref(
                    "turn",
                    &format!("{}:{}", event.conversation_id, event.turn_sequence),
                ),
                role: event.role,
                event_kind: "message",
                content: String::new(),
                created_at: format_millis(event.created_at_ms, timezone),
                truncated: false,
            },
            0,
        ));
    }
    if allowance == 0 {
        return None;
    }
    let (content, truncated) = if event.content.len() <= allowance {
        (event.content, false)
    } else if allowance >= '…'.len_utf8() {
        let content_allowance = allowance - '…'.len_utf8();
        let mut content = String::new();
        for character in event.content.chars() {
            if content.len().saturating_add(character.len_utf8()) > content_allowance {
                break;
            }
            content.push(character);
        }
        content.push('…');
        (content, true)
    } else {
        return None;
    };
    let used = content.len();
    Some((
        RecallEvent {
            event_ref: opaque_ref("event", &event.id),
            turn_ref: opaque_ref(
                "turn",
                &format!("{}:{}", event.conversation_id, event.turn_sequence),
            ),
            role: event.role,
            event_kind: "message",
            content,
            created_at: format_millis(event.created_at_ms, timezone),
            truncated,
        },
        used,
    ))
}

pub(super) fn public_range(range: &ResolvedRange) -> ResolvedTimeRange {
    ResolvedTimeRange {
        from: format_millis(range.from_ms, range.timezone),
        to_exclusive: format_millis(range.to_exclusive_ms, range.timezone),
        timezone: range.timezone.name().to_string(),
        label: range.label.clone(),
    }
}

pub(super) fn format_millis(milliseconds: i64, timezone: Tz) -> String {
    DateTime::<Utc>::from_timestamp_millis(milliseconds)
        .map(|value| value.with_timezone(&timezone).to_rfc3339())
        .unwrap_or_else(|| "invalid-timestamp".to_string())
}

pub(super) fn persist_receipt(
    connection: &mut Connection,
    context: &RecallExecutionContext<'_>,
    query: Option<&str>,
    range: Option<&ResolvedRange>,
    call_index: i64,
    output: &RecallConversationOutput,
) -> Result<(), RecallError> {
    let matched = output
        .windows
        .iter()
        .flat_map(|window| window.matched_event_refs.iter())
        .cloned()
        .collect::<Vec<_>>();
    let matched_json = serde_json::to_string(&matched).map_err(|_| local_unavailable(""))?;
    let transaction = connection.transaction().map_err(local_unavailable)?;
    transaction
        .execute(
            "INSERT INTO conversation_recall_receipts(
               id,runtime_run_id,tool_call_id,call_index,query_digest,range_from_ms,
               range_to_exclusive_ms,timezone,matched_event_refs_json,reason_code,created_at_ms
             ) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)",
            params![
                format!("recall_receipt_{}", uuid::Uuid::new_v4().simple()),
                context.runtime_run_id,
                context.tool_call_id,
                call_index,
                query.map(digest),
                range.map(|value| value.from_ms),
                range.map(|value| value.to_exclusive_ms),
                range.map(|value| value.timezone.name()),
                matched_json,
                output.reason_code,
                context.now.timestamp_millis(),
            ],
        )
        .map_err(local_unavailable)?;
    transaction.commit().map_err(local_unavailable)
}

pub(super) fn escape_like(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_")
}

pub(super) fn score(raw_bm25: f64) -> f64 {
    if !raw_bm25.is_finite() {
        return 0.0;
    }
    let strength = (-raw_bm25).max(0.0);
    (strength / (1.0 + strength)).clamp(0.0, 1.0)
}

pub(super) fn digest(value: &str) -> String {
    Sha256::digest(value.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

pub(super) fn opaque_ref(prefix: &str, value: &str) -> String {
    format!("{prefix}_{}", &digest(value)[..24])
}

pub(super) fn local_unavailable<E>(_error: E) -> RecallError {
    RecallError::new(
        RecallErrorCode::LocalRecallUnavailable,
        "Local conversation recall is temporarily unavailable.",
    )
}
