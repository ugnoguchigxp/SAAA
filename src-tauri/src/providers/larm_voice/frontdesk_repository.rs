//! Durable ASR delivery and one-shot LFM request for Qwen reasoning. User text is never rewritten.
use crate::{database_error, new_id, now_iso};
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or(0)
}

pub(crate) fn migrate(c: &Connection) -> rusqlite::Result<()> {
    c.execute_batch(
        "CREATE TABLE IF NOT EXISTS lfm_voice_utterances (
        utterance_id TEXT PRIMARY KEY,
        conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
        user_message_id TEXT NOT NULL REFERENCES conversation_messages(id) ON DELETE CASCADE,
        reply_message_id TEXT REFERENCES conversation_messages(id) ON DELETE CASCADE,
        status TEXT NOT NULL CHECK(status IN ('pending','respond','delegate','failed')),
        reasoning_request_id TEXT UNIQUE,
        request_message_id TEXT REFERENCES conversation_messages(id) ON DELETE CASCADE,
        claimed_run_id TEXT,
        failure_code TEXT
    );",
    )?;
    let has_reasoning_request: bool = c.query_row(
        "SELECT EXISTS(SELECT 1 FROM pragma_table_info('lfm_voice_utterances') WHERE name='reasoning_request_id')",
        [], |row| row.get(0),
    )?;
    if !has_reasoning_request {
        c.execute_batch("ALTER TABLE lfm_voice_utterances ADD COLUMN reasoning_request_id TEXT")?;
    }
    let has_legacy_handoff: bool = c.query_row(
        "SELECT EXISTS(SELECT 1 FROM pragma_table_info('lfm_voice_utterances') WHERE name='handoff_id')",
        [], |row| row.get(0),
    )?;
    if has_legacy_handoff {
        c.execute_batch("UPDATE lfm_voice_utterances SET reasoning_request_id=handoff_id WHERE reasoning_request_id IS NULL AND handoff_id IS NOT NULL")?;
    }
    c.execute_batch("CREATE UNIQUE INDEX IF NOT EXISTS idx_lfm_voice_reasoning_request ON lfm_voice_utterances(reasoning_request_id)")
}

#[derive(Debug)]
pub(crate) enum AcceptOutcome {
    Process,
    Completed {
        reasoning_request_id: Option<String>,
        request_content: Option<String>,
    },
}

struct ExistingUtterance {
    conversation_id: String,
    input_text: String,
    status: String,
    reply_message_id: Option<String>,
    reasoning_request_id: Option<String>,
    request_content: Option<String>,
    claimed_run_id: Option<String>,
}

fn validate_frontend_binding(c: &Connection) -> Result<(), String> {
    let routing = crate::persistence::load_role_routing_settings(c)?;
    if !routing.enabled {
        return Err("role-routing-disabled".into());
    }
    let frontend_id = routing
        .roles
        .frontend
        .as_deref()
        .ok_or("role-routing-frontend-not-configured")?;
    let frontend = routing
        .actors
        .iter()
        .find(|actor| actor.id == frontend_id)
        .ok_or("role-routing-frontend-actor-missing")?;
    if frontend.transport != "provider"
        || frontend.provider_id.as_deref() != Some(crate::DYNAMIC_LAN_PROVIDER_ID)
        || !frontend
            .capabilities
            .iter()
            .any(|capability| capability == "social_reply")
    {
        return Err("role-routing-frontend-binding-mismatch".into());
    }
    Ok(())
}

pub(crate) fn accept(
    c: &Connection,
    conversation: &str,
    utterance: &str,
    text: &str,
) -> Result<AcceptOutcome, String> {
    let tx = c.unchecked_transaction().map_err(database_error)?;
    validate_frontend_binding(&tx)?;
    let existing: Option<ExistingUtterance> = tx
        .query_row(
            "SELECT u.conversation_id,input.content,u.status,u.reply_message_id,
                    u.reasoning_request_id,request.content,u.claimed_run_id
             FROM lfm_voice_utterances u
             JOIN conversation_messages input ON input.id=u.user_message_id
             LEFT JOIN conversation_messages request ON request.id=u.request_message_id
             WHERE u.utterance_id=?1",
            [utterance],
            |row| {
                Ok(ExistingUtterance {
                    conversation_id: row.get(0)?,
                    input_text: row.get(1)?,
                    status: row.get(2)?,
                    reply_message_id: row.get(3)?,
                    reasoning_request_id: row.get(4)?,
                    request_content: row.get(5)?,
                    claimed_run_id: row.get(6)?,
                })
            },
        )
        .optional()
        .map_err(database_error)?;
    if let Some(existing) = existing {
        if existing.conversation_id != conversation || existing.input_text != text {
            return Err("lfm-utterance-conflicts-with-original".into());
        }
        if matches!(existing.status.as_str(), "respond" | "delegate") {
            if existing.reply_message_id.is_none()
                || (existing.status == "delegate" && existing.request_content.is_none())
            {
                return Err("lfm-utterance-completed-state-invalid".into());
            }
            let (reasoning_request_id, request_content) = if existing.claimed_run_id.is_some() {
                (None, None)
            } else {
                (existing.reasoning_request_id, existing.request_content)
            };
            return Ok(AcceptOutcome::Completed {
                reasoning_request_id,
                request_content,
            });
        }
        tx.execute(
            "UPDATE lfm_voice_utterances SET status='pending',failure_code=NULL WHERE utterance_id=?1",
            [utterance],
        ).map_err(database_error)?;
        tx.commit().map_err(database_error)?;
        return Ok(AcceptOutcome::Process);
    }
    let id = new_id("message");
    tx.execute("INSERT INTO conversation_messages(id,conversation_id,role,content,created_at) VALUES(?1,?2,'user',?3,?4)", params![id,conversation,text,now_iso()]).map_err(database_error)?;
    tx.execute("INSERT INTO lfm_voice_utterances(utterance_id,conversation_id,user_message_id,status) VALUES(?1,?2,?3,'pending')",params![utterance,conversation,id]).map_err(database_error)?;
    let digest = format!("{:x}", Sha256::digest(text.as_bytes()));
    crate::role_routing::repository::record_input_receipt(
        &tx,
        None,
        &format!("rr-voice-{utterance}"),
        conversation,
        &id,
        &digest,
        Some(utterance),
        "voice",
        "frontend_pending",
        0,
        now_ms(),
    )?;
    tx.execute(
        "UPDATE conversations SET updated_at=?2 WHERE id=?1",
        params![conversation, now_iso()],
    )
    .map_err(database_error)?;
    tx.commit().map_err(database_error)?;
    Ok(AcceptOutcome::Process)
}

pub(crate) fn context(c: &Connection, conversation: &str) -> Result<(Vec<Value>, bool), String> {
    let mut stmt = c.prepare("SELECT role,content FROM (SELECT rowid AS ordinal,role,content FROM conversation_messages WHERE conversation_id=?1 AND role IN ('user','assistant') ORDER BY rowid DESC LIMIT 24) ORDER BY ordinal").map_err(database_error)?;
    let rows = stmt
        .query_map([conversation], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })
        .map_err(database_error)?;
    let mut history = Vec::new();
    for row in rows {
        let (role, content) = row.map_err(database_error)?;
        history.push(json!({"role":role,"content":content}));
    }
    let pending = c.query_row("SELECT EXISTS(SELECT 1 FROM lfm_voice_utterances u LEFT JOIN runtime_runs r ON r.id=u.claimed_run_id WHERE u.conversation_id=?1 AND u.status='delegate' AND (u.claimed_run_id IS NULL OR r.status='running'))",[conversation],|r|r.get(0)).map_err(database_error)?;
    Ok((history, pending))
}

pub(crate) fn complete(
    c: &Connection,
    utterance: &str,
    decision: &super::frontdesk_decision::ConversationDecision,
) -> Result<Option<(String, String)>, String> {
    let tx = c.unchecked_transaction().map_err(database_error)?;
    let conversation: String = tx.query_row("SELECT conversation_id FROM lfm_voice_utterances WHERE utterance_id=?1 AND status='pending'",[utterance],|r|r.get(0)).map_err(database_error)?;
    let reply_id = new_id("lfm_reply");
    let request_reasoning = decision.think;
    let reasoning_request = request_reasoning.then(|| new_id("lfm_reasoning"));
    let request = if request_reasoning {
        // Keep every segment of the current request verbatim. LFM cannot rewrite user authority.
        let mut statement = tx.prepare("SELECT m.content FROM lfm_voice_utterances u JOIN conversation_messages m ON m.id=u.user_message_id WHERE u.conversation_id=?1 AND u.rowid <= (SELECT rowid FROM lfm_voice_utterances WHERE utterance_id=?2) AND u.rowid > COALESCE((SELECT MAX(rowid) FROM lfm_voice_utterances WHERE conversation_id=?1 AND status='delegate'),0) ORDER BY u.rowid").map_err(database_error)?;
        let segments = statement
            .query_map(params![conversation, utterance], |r| r.get::<_, String>(0))
            .map_err(database_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(database_error)?;
        let content = segments.join("\n");
        if content.chars().count() > 16_000 {
            return Err("lfm-request-context-too-large".into());
        }
        let id = new_id("lfm_request");
        tx.execute("INSERT INTO conversation_messages(id,conversation_id,role,content,created_at) VALUES(?1,?2,'user',?3,?4)",params![id,conversation,content,now_iso()]).map_err(database_error)?;
        Some((id, content))
    } else {
        None
    };
    tx.execute("INSERT INTO conversation_messages(id,conversation_id,role,content,created_at) VALUES(?1,?2,'assistant',?3,?4)",params![reply_id,conversation,decision.say,now_iso()]).map_err(database_error)?;
    tx.execute("UPDATE lfm_voice_utterances SET reply_message_id=?2,status=?3,reasoning_request_id=?4,request_message_id=?5 WHERE utterance_id=?1",params![utterance,reply_id,if request_reasoning {"delegate"} else {"respond"},reasoning_request,request.as_ref().map(|r|&r.0)]).map_err(database_error)?;
    tx.execute(
        "UPDATE rr_inputs SET disposition=?2 WHERE message_id=(SELECT user_message_id FROM lfm_voice_utterances WHERE utterance_id=?1) AND root_id IS NULL",
        params![utterance, if request_reasoning {"reasoning_requested"} else {"frontend_completed"}],
    ).map_err(database_error)?;
    tx.commit().map_err(database_error)?;
    Ok(reasoning_request.zip(request.map(|r| r.1)))
}

/// Called inside prepare_runtime_run's transaction. A duplicate cannot launch a second Qwen run.
pub(crate) fn claim_reasoning_request(
    c: &Connection,
    input: &crate::StartTurnInput,
) -> Result<Option<String>, String> {
    let Some(source) = input
        .source_id
        .as_deref()
        .filter(|s| is_reasoning_request_id(s))
    else {
        return Ok(None);
    };
    let message: Option<String> = c.query_row("SELECT u.request_message_id FROM lfm_voice_utterances u JOIN conversation_messages m ON m.id=u.request_message_id WHERE u.reasoning_request_id=?1 AND u.conversation_id=?2 AND u.status='delegate' AND u.claimed_run_id IS NULL AND m.content=?3",params![source,input.conversation_id,input.content.trim()],|r|r.get(0)).optional().map_err(database_error)?;
    let message = message.ok_or("lfm-reasoning-request-invalid-or-already-claimed")?;
    c.execute(
        "UPDATE lfm_voice_utterances SET claimed_run_id=?2 WHERE reasoning_request_id=?1",
        params![source, input.run_id],
    )
    .map_err(database_error)?;
    Ok(Some(message))
}

pub(crate) fn is_reasoning_request_id(source_id: &str) -> bool {
    source_id.starts_with("lfm_reasoning_") || source_id.starts_with("lfm_handoff_")
}
