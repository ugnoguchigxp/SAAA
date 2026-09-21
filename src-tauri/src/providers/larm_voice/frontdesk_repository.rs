//! Durable ASR delivery and one-shot LFM -> Qwen handoff. User text is never rewritten.
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};
use crate::{database_error, new_id, now_iso};

pub(crate) fn migrate(c: &Connection) -> rusqlite::Result<()> {
    c.execute_batch("CREATE TABLE IF NOT EXISTS lfm_voice_utterances (
        utterance_id TEXT PRIMARY KEY,
        conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
        user_message_id TEXT NOT NULL REFERENCES conversation_messages(id) ON DELETE CASCADE,
        reply_message_id TEXT REFERENCES conversation_messages(id) ON DELETE CASCADE,
        status TEXT NOT NULL CHECK(status IN ('pending','respond','delegate','failed')),
        handoff_id TEXT UNIQUE,
        request_message_id TEXT REFERENCES conversation_messages(id) ON DELETE CASCADE,
        claimed_run_id TEXT,
        failure_code TEXT
    );")
}

pub(crate) fn accept(c: &Connection, conversation: &str, utterance: &str, text: &str) -> Result<(), String> {
    let tx = c.unchecked_transaction().map_err(database_error)?;
    let exists: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM lfm_voice_utterances WHERE utterance_id=?1)", [utterance], |r| r.get(0)).map_err(database_error)?;
    if exists { return Err("lfm-utterance-already-received".into()); }
    let id = new_id("message");
    tx.execute("INSERT INTO conversation_messages(id,conversation_id,role,content,created_at) VALUES(?1,?2,'user',?3,?4)", params![id,conversation,text,now_iso()]).map_err(database_error)?;
    tx.execute("INSERT INTO lfm_voice_utterances(utterance_id,conversation_id,user_message_id,status) VALUES(?1,?2,?3,'pending')",params![utterance,conversation,id]).map_err(database_error)?;
    tx.execute("UPDATE conversations SET updated_at=?2 WHERE id=?1",params![conversation,now_iso()]).map_err(database_error)?;
    tx.commit().map_err(database_error)
}

pub(crate) fn context(c: &Connection, conversation: &str) -> Result<(Vec<Value>, bool), String> {
    let mut stmt = c.prepare("SELECT role,content FROM (SELECT rowid AS ordinal,role,content FROM conversation_messages WHERE conversation_id=?1 AND role IN ('user','assistant') ORDER BY rowid DESC LIMIT 24) ORDER BY ordinal").map_err(database_error)?;
    let rows = stmt.query_map([conversation], |r| Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?))).map_err(database_error)?;
    let mut history = Vec::new();
    for row in rows {
        let (role, content) = row.map_err(database_error)?;
        history.push(json!({"role":role,"content":content}));
    }
    let pending = c.query_row("SELECT EXISTS(SELECT 1 FROM lfm_voice_utterances u LEFT JOIN runtime_runs r ON r.id=u.claimed_run_id WHERE u.conversation_id=?1 AND u.status='delegate' AND (u.claimed_run_id IS NULL OR r.status='running'))",[conversation],|r|r.get(0)).map_err(database_error)?;
    Ok((history,pending))
}

pub(crate) fn complete(c: &Connection, utterance: &str, decision: &super::frontdesk_decision::ConversationDecision) -> Result<Option<(String,String)>, String> {
    let tx = c.unchecked_transaction().map_err(database_error)?;
    let conversation: String = tx.query_row("SELECT conversation_id FROM lfm_voice_utterances WHERE utterance_id=?1 AND status='pending'",[utterance],|r|r.get(0)).map_err(database_error)?;
    let reply_id = new_id("lfm_reply");
    let delegate = decision.action == super::frontdesk_decision::ConversationAction::Delegate;
    let handoff = delegate.then(|| new_id("lfm_handoff"));
    let request = if delegate {
        // Keep every segment of the current request verbatim. LFM cannot rewrite user authority.
        let mut statement = tx.prepare("SELECT m.content FROM lfm_voice_utterances u JOIN conversation_messages m ON m.id=u.user_message_id WHERE u.conversation_id=?1 AND u.rowid <= (SELECT rowid FROM lfm_voice_utterances WHERE utterance_id=?2) AND u.rowid > COALESCE((SELECT MAX(rowid) FROM lfm_voice_utterances WHERE conversation_id=?1 AND status='delegate'),0) ORDER BY u.rowid").map_err(database_error)?;
        let segments = statement.query_map(params![conversation,utterance],|r|r.get::<_,String>(0)).map_err(database_error)?.collect::<Result<Vec<_>,_>>().map_err(database_error)?;
        let content = segments.join("\n");
        if content.chars().count() > 16_000 {return Err("lfm-request-context-too-large".into());}
        let id = new_id("lfm_request");
        tx.execute("INSERT INTO conversation_messages(id,conversation_id,role,content,created_at) VALUES(?1,?2,'user',?3,?4)",params![id,conversation,content,now_iso()]).map_err(database_error)?;
        Some((id,content))
    } else {None};
    tx.execute("INSERT INTO conversation_messages(id,conversation_id,role,content,created_at) VALUES(?1,?2,'assistant',?3,?4)",params![reply_id,conversation,decision.reply,now_iso()]).map_err(database_error)?;
    tx.execute("UPDATE lfm_voice_utterances SET reply_message_id=?2,status=?3,handoff_id=?4,request_message_id=?5 WHERE utterance_id=?1",params![utterance,reply_id,if delegate {"delegate"} else {"respond"},handoff,request.as_ref().map(|r|&r.0)]).map_err(database_error)?;
    tx.commit().map_err(database_error)?;
    Ok(handoff.zip(request.map(|r|r.1)))
}

/// Called inside prepare_runtime_run's transaction. A duplicate cannot launch a second Qwen run.
pub(crate) fn claim_handoff(c: &Connection, input: &crate::StartTurnInput) -> Result<Option<String>,String> {
    let Some(source) = input.source_id.as_deref().filter(|s|s.starts_with("lfm_handoff_")) else { return Ok(None); };
    let message: Option<String> = c.query_row("SELECT u.request_message_id FROM lfm_voice_utterances u JOIN conversation_messages m ON m.id=u.request_message_id WHERE u.handoff_id=?1 AND u.conversation_id=?2 AND u.status='delegate' AND u.claimed_run_id IS NULL AND m.content=?3",params![source,input.conversation_id,input.content.trim()],|r|r.get(0)).optional().map_err(database_error)?;
    let message = message.ok_or("lfm-handoff-invalid-or-already-claimed")?;
    c.execute("UPDATE lfm_voice_utterances SET claimed_run_id=?2 WHERE handoff_id=?1",params![source,input.run_id]).map_err(database_error)?;
    Ok(Some(message))
}
