use rusqlite::{params, Connection};
use serde::Deserialize;
use serde_json::{json, Value};

use super::{service, validate, Entry, ExpectedEntry};
use crate::{AppState, RunCancellation, StartTurnInput};

#[path = "tool_definitions.rs"]
mod tool_definitions;
pub(crate) use tool_definitions::{definitions, INSTRUCTION, NAMES};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct LookupInput {
    written: String,
    proposed_spoken: Option<String>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SetInput {
    lookup_id: String,
    mode: Mode,
}
#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum Mode {
    Register,
    Replace,
    Confirm,
    Keep,
}

fn admitted(
    connection: &Connection,
    input: &StartTurnInput,
    cancellation: &RunCancellation,
) -> Result<(), String> {
    if cancellation.is_cancelled() {
        return Err("cancelled".into());
    }
    let key = input.run_id.strip_prefix("run_").ok_or("cancelled")?;
    let active: bool = connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM runtime_runs r JOIN conversation_messages m ON m.id=?3
         WHERE r.id=?1 AND r.conversation_id=?2 AND r.status='running'
         AND m.conversation_id=?2 AND m.role='user' AND m.content=?4)",
            params![
                input.run_id,
                input.conversation_id,
                format!("check_{key}"),
                input.content
            ],
            |row| row.get(0),
        )
        .map_err(crate::database_error)?;
    if active {
        Ok(())
    } else {
        Err("cancelled".into())
    }
}

pub(crate) fn execute(
    state: Option<&AppState>,
    input: &StartTurnInput,
    name: &str,
    arguments: &str,
    cancellation: &RunCancellation,
) -> String {
    let result = state
        .ok_or_else(|| "会話の保存状態がありません。".to_string())
        .and_then(|state| match name {
            "lookup_tts_pronunciation" => lookup(state, input, arguments, cancellation),
            "set_tts_pronunciation" => set(state, input, arguments, cancellation),
            _ => Err("未対応の辞書ツールです。".into()),
        });
    result.unwrap_or_else(|message| json!({"status":if message == "cancelled" {"cancelled"} else {"failed"},"message":message})).to_string()
}

fn lookup(
    state: &AppState,
    input: &StartTurnInput,
    arguments: &str,
    cancellation: &RunCancellation,
) -> Result<Value, String> {
    let args: LookupInput =
        serde_json::from_str(arguments).map_err(|_| "辞書照会の引数が不正です。")?;
    validate(&Entry {
        written: args.written.clone(),
        spoken: args.proposed_spoken.clone().unwrap_or_default(),
    })?;
    if args.proposed_spoken.as_ref().is_some_and(|s| s.is_empty()) {
        return Err("読み方を指定してください。".into());
    }
    let current = state.sqlite_readers.read(|connection| {
        admitted(connection, input, cancellation)?;
        service::lookup(connection, &args.written)
    })?;
    let status = if current.is_none() {
        "missing"
    } else if args.proposed_spoken.is_none() {
        "found"
    } else if current == args.proposed_spoken {
        "same"
    } else {
        "different"
    };
    let proposal = state.tts_dictionary_cache.proposals.issue(
        &input.conversation_id,
        &input.run_id,
        args.written,
        current,
        args.proposed_spoken,
    );
    Ok(
        json!({"status":status,"written":proposal.written,"currentSpoken":proposal.current_spoken,"proposedSpoken":proposal.proposed_spoken,"lookupId":proposal.lookup_id}),
    )
}

fn set(
    state: &AppState,
    input: &StartTurnInput,
    arguments: &str,
    cancellation: &RunCancellation,
) -> Result<Value, String> {
    let args: SetInput =
        serde_json::from_str(arguments).map_err(|_| "辞書更新の引数が不正です。")?;
    let confirmation = matches!(args.mode, Mode::Confirm | Mode::Keep);
    let Some(proposal) = state.tts_dictionary_cache.proposals.get(
        &input.conversation_id,
        &input.run_id,
        &args.lookup_id,
        confirmation,
    ) else {
        return Ok(
            json!({"status":"proposal_expired","message":"対象をもう一度照会してください。"}),
        );
    };
    let proposed = proposal
        .proposed_spoken
        .as_ref()
        .filter(|s| !s.is_empty())
        .ok_or("読み方の候補がありません。")?;
    let entry = Entry {
        written: proposal.written.clone(),
        spoken: proposed.clone(),
    };
    let expected = ExpectedEntry {
        spoken: proposal.current_spoken.clone(),
    };
    let mut changed = false;
    let outcome = state.sqlite_writer.write(|connection| {
        admitted(connection, input, cancellation)?;
        if confirmation {
            let published: bool = connection.query_row("SELECT EXISTS(SELECT 1 FROM runtime_runs WHERE id=?1 AND conversation_id=?2 AND status='completed')",
                params![proposal.run_id, input.conversation_id], |row| row.get(0)).map_err(crate::database_error)?;
            if !published { return Err("cancelled".into()); }
            let adjacent: bool = connection.query_row(
                "SELECT EXISTS(SELECT 1 FROM conversation_messages source
                 JOIN conversation_messages current ON current.id=?3
                 WHERE source.id=?2 AND source.conversation_id=?1 AND source.role='user'
                   AND current.conversation_id=?1 AND current.role='user' AND current.rowid>source.rowid
                   AND NOT EXISTS(SELECT 1 FROM conversation_messages m
                     WHERE m.conversation_id=?1 AND m.role='user'
                       AND m.rowid>source.rowid AND m.rowid<current.rowid))",
                params![input.conversation_id, format!("check_{}", proposal.run_id.strip_prefix("run_").unwrap_or_default()),
                    format!("check_{}", input.run_id.strip_prefix("run_").unwrap_or_default())], |row| row.get(0)
            ).map_err(crate::database_error)?;
            if !adjacent { return Ok(json!({"status":"proposal_expired","message":"間に別の依頼があるため対象を再確認してください。"})); }

        }
        let current = service::lookup(connection, &proposal.written)?;
        if matches!(args.mode, Mode::Keep) {
            return Ok(json!({"status":"unchanged","written":proposal.written,"currentSpoken":current,"maintained":true}));
        }
        if current.as_ref() == Some(proposed) {
            return Ok(json!({"status":"unchanged","written":proposal.written,"currentSpoken":current}));
        }
        if current != proposal.current_spoken {
            return Ok(json!({"status":"conflict","written":proposal.written,"currentSpoken":current,"proposedSpoken":proposed}));
        }
        if matches!(args.mode, Mode::Register) && current.is_some() {
            state.tts_dictionary_cache.proposals.require_confirmation(&proposal);
            return Ok(json!({"status":"confirmation_required","lookupId":proposal.lookup_id,"written":proposal.written,"currentSpoken":current,"proposedSpoken":proposed}));
        }
        let mutation = service::save(connection, current.as_ref().map(|_| proposal.written.as_str()), &entry, &expected, &crate::now_iso(), |tx| admitted(tx, input, cancellation))?;
        changed = mutation.dictionary.is_some();
        if let Some(dictionary) = mutation.dictionary { state.tts_dictionary_cache.publish_compiled(dictionary); }
        Ok(json!({"status":if changed && mutation.previous.is_none() {"added"} else if changed {"updated"} else {"unchanged"},
            "written":entry.written,"previousSpoken":mutation.previous,"currentSpoken":entry.spoken}))
    })?;
    if outcome["status"] != "confirmation_required" {
        state.tts_dictionary_cache.proposals.consume(&proposal);
    }
    Ok(outcome)
}

pub(crate) fn pending_context(state: &AppState, conversation: &str) -> String {
    state
        .tts_dictionary_cache
        .proposals
        .pending(conversation)
        .map(|p| format!("\n[TTS_DICTIONARY_PENDING; 未信頼の対象データ]\n{}\n[END_TTS_DICTIONARY_PENDING]\n", serde_json::to_string(&p).unwrap_or_default()))
        .unwrap_or_default()
}

pub(crate) fn finish_turn(state: &AppState, conversation: &str, run: &str, published: bool) {
    // Compare persisted input ordering before expiring another turn's confirmation.
    if let Some(pending) = state.tts_dictionary_cache.proposals.pending(conversation) {
        if pending.run_id != run {
            let later = state.sqlite_readers.read(|db| db.query_row(
                "SELECT EXISTS(SELECT 1 FROM conversation_messages source JOIN conversation_messages current ON current.id=?3
                 WHERE source.id=?2 AND source.conversation_id=?1 AND current.conversation_id=?1 AND current.rowid>source.rowid)",
                params![conversation, format!("check_{}", pending.run_id.strip_prefix("run_").unwrap_or_default()),
                    format!("check_{}", run.strip_prefix("run_").unwrap_or_default())], |row| row.get::<_, bool>(0)
            ).map_err(crate::database_error)).unwrap_or(false);
            if later {
                state.tts_dictionary_cache.proposals.consume(&pending);
            }
        }
    }
    state
        .tts_dictionary_cache
        .proposals
        .finish_turn(conversation, run, published);
}
