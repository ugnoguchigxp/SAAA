//! The terminal transaction and the steward outbox hand-off for asynchronous results.
//!
//! The report text is a host template: no model-authored text reaches the speech/outbox path.
//! `steward::repository` is private to the steward module, so the outbox row is written here with
//! the same columns `enqueue_task_report` writes (`destination='conversation'`, unique on
//! `(task_id, task_revision, destination)`).
use super::store::{self, db, ACTIVE_STATES};
use crate::worker_agents::contracts::*;
use rusqlite::{params, Connection};

#[derive(Debug, Clone, PartialEq)]
pub(super) enum Terminal {
    Succeeded(WorkerOutput),
    Failed(FailureCode),
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Finalized {
    pub conversation_id: String,
    /// The row moved to a terminal state in this call (false: it already was terminal).
    pub changed: bool,
    /// A steward outbox row was enqueued; the caller must flush it after commit.
    pub reported: bool,
}

fn host_of(url: &str) -> String {
    url::Url::parse(url)
        .ok()
        .and_then(|parsed| {
            parsed
                .host_str()
                .map(|host| host.trim_start_matches("www.").to_string())
        })
        .unwrap_or_else(|| "不明".to_string())
}

fn failure_phrase(code: FailureCode) -> &'static str {
    match code {
        FailureCode::NoSafeSources => "安全に確認できる情報源が見つからず、確認できませんでした。",
        FailureCode::NoResults => "該当する情報が見つかりませんでした。",
        FailureCode::DeadlineExceeded => "時間内に確認できませんでした。",
        FailureCode::BudgetExhausted => "確認の上限に達したため、確認できませんでした。",
        FailureCode::Interrupted => "処理が中断されたため、確認できませんでした。",
        FailureCode::EscalationRequiresApproval => {
            "外部の高性能モデルの利用に承認が必要なため、確認できませんでした。"
        }
        FailureCode::OutcomeUnknown => "結果を確実に確認できませんでした。",
        _ => "確認できませんでした。",
    }
}

/// Spoken/written digest built only from host-verified fields.
pub(super) fn digest_for(terminal: &Terminal) -> String {
    match terminal {
        Terminal::Succeeded(WorkerOutput::WebClaimsV1(claims)) => {
            let mut text = String::from("先ほどの調査結果です（Web由来の情報）。");
            for claim in claims.claims.iter().take(3) {
                text.push_str(&claim.text);
                text.push('（');
                text.push_str(&host_of(&claim.source_url));
                text.push('）');
            }
            text
        }
        Terminal::Succeeded(WorkerOutput::JsonV1(_)) => "先ほどの依頼が完了しました。".to_string(),
        Terminal::Failed(code) => failure_phrase(*code).to_string(),
        Terminal::Cancelled => String::new(),
    }
}

/// Inserts the outbox row for `task_id`. Idempotent: the unique index on
/// `(task_id, task_revision, destination)` makes a repeated call a no-op.
pub(super) fn enqueue_report(
    connection: &Connection,
    conversation_id: &str,
    task_id: &str,
    digest: &str,
    available_at_ms: i64,
) -> Result<bool, String> {
    let inserted = connection
        .execute(
            "INSERT OR IGNORE INTO steward_reports(id, conversation_id, digest, held_reason, flushed,
                created_at, available_at_ms, task_id, task_revision, destination, delivery_state,
                speak_requested, speech_state)
             VALUES(?1, ?2, ?3, NULL, 0, ?4, ?5, ?6, 0, 'conversation', 'pending', 1, 'pending')",
            params![
                crate::new_id("report"),
                conversation_id,
                digest,
                crate::now_iso(),
                available_at_ms,
                task_id
            ],
        )
        .map_err(db)?;
    Ok(inserted == 1)
}

/// Moves an active task to its terminal state. Safe to invoke any number of times: only the
/// first call changes the row, and the outbox row is unique per task. A task that was cancelled
/// in the meantime stays cancelled.
pub(super) fn finalize(
    connection: &Connection,
    task_id: &str,
    terminal: &Terminal,
    now_ms: i64,
) -> Result<Finalized, String> {
    let row = store::load_task(connection, task_id)?
        .ok_or_else(|| "worker task not found".to_string())?;
    let mut finalized = Finalized {
        conversation_id: row.conversation_id.clone(),
        changed: false,
        reported: false,
    };
    if !store::is_active(&row.state) {
        return Ok(finalized);
    }
    let (state, result_json, failure_code, delivery) = match terminal {
        Terminal::Succeeded(output) => (
            "succeeded",
            Some(serde_json::to_string(output).map_err(|error| error.to_string())?),
            None,
            row.delivery.as_str(),
        ),
        Terminal::Failed(code) => ("failed", None, Some(code.as_str()), row.delivery.as_str()),
        Terminal::Cancelled => ("cancelled", None, Some("cancelled"), "suppressed"),
    };
    let changed = connection
        .execute(
            &format!(
                "UPDATE worker_tasks SET state = ?2, result_json = ?3, failure_code = ?4,
                    delivery = ?5, updated_at_ms = ?6
                 WHERE id = ?1 AND state IN {ACTIVE_STATES}"
            ),
            params![task_id, state, result_json, failure_code, delivery, now_ms],
        )
        .map_err(db)?;
    finalized.changed = changed == 1;
    if finalized.changed && delivery == "async_queued" {
        finalized.reported = enqueue_report(
            connection,
            &row.conversation_id,
            task_id,
            &digest_for(terminal),
            now_ms,
        )?;
        connection
            .execute(
                "UPDATE worker_tasks SET delivery = 'async_delivered' WHERE id = ?1",
                params![task_id],
            )
            .map_err(db)?;
    }
    Ok(finalized)
}
