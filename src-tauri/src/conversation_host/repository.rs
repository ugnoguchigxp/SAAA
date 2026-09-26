//! The preview ledger is separate from legacy runtime runs and never stores credentials.
use rusqlite::{params, Connection, OptionalExtension};
use saaa_conversation_core::contracts::{validate_input, Action, TextInput};
use saaa_conversation_core::frontdesk::DELEGATE_EXPLANATION;
use sha2::{Digest, Sha256};

pub(crate) fn migrate(db: &Connection) -> rusqlite::Result<()> {
    db.execute_batch(
        "CREATE TABLE IF NOT EXISTS conversation_preview_sessions (
           session_id TEXT PRIMARY KEY,
           conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
           config_fingerprint TEXT NOT NULL,
           resource_state TEXT NOT NULL CHECK(resource_state IN
             ('preparing','ready','closing','closed','failed','cleanup_pending')),
           resource_generation INTEGER NOT NULL DEFAULT 0,
           larm_key TEXT NOT NULL UNIQUE,
           connection_id TEXT,
           cleanup_state TEXT NOT NULL CHECK(cleanup_state IN
             ('none','pending','released','failed')),
           created_at TEXT NOT NULL,
           updated_at TEXT NOT NULL
         );
         CREATE TABLE IF NOT EXISTS conversation_preview_responses (
           response_id TEXT PRIMARY KEY,
           session_id TEXT NOT NULL REFERENCES conversation_preview_sessions(session_id) ON DELETE CASCADE,
           input_id TEXT NOT NULL,
           input_hash BLOB NOT NULL CHECK(length(input_hash) = 32),
           user_message_id TEXT NOT NULL REFERENCES conversation_messages(id),
           assistant_message_id TEXT REFERENCES conversation_messages(id),
           action TEXT CHECK(action IS NULL OR action IN ('reply','clarify','delegate')),
           frontdesk_state TEXT NOT NULL CHECK(frontdesk_state IN
             ('awaiting_control','streaming','completed','unsupported','failed','cancelled','interrupted')),
           speech_id TEXT NOT NULL UNIQUE,
           speech_generation INTEGER NOT NULL DEFAULT 0,
           speech_state TEXT NOT NULL CHECK(speech_state IN
             ('idle','collecting','synthesizing','playing','stopping','played','stopped','failed','interrupted')),
           public_text TEXT NOT NULL DEFAULT '',
           last_persisted_clause INTEGER,
           last_played_clause INTEGER,
           failure_stage TEXT,
           failure_code TEXT,
           version INTEGER NOT NULL DEFAULT 1,
           created_at TEXT NOT NULL,
           updated_at TEXT NOT NULL,
           UNIQUE(session_id,input_id)
         );
         CREATE INDEX IF NOT EXISTS idx_conversation_preview_responses_session
           ON conversation_preview_responses(session_id, created_at);
         CREATE TABLE IF NOT EXISTS conversation_preview_events (
           session_id TEXT NOT NULL REFERENCES conversation_preview_sessions(session_id) ON DELETE CASCADE,
           event_seq INTEGER NOT NULL,
           response_id TEXT REFERENCES conversation_preview_responses(response_id),
           kind TEXT NOT NULL,
           wall_time TEXT NOT NULL,
           elapsed_ms INTEGER,
           request_id TEXT,
           connection_id TEXT,
           allocation_id TEXT,
           detail TEXT,
           PRIMARY KEY(session_id,event_seq)
         );",
    )
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Receipt {
    pub response_id: String,
    pub input_id: String,
    pub duplicate: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum AcceptError {
    InvalidInput,
    SessionClosed,
    Conflict,
    Busy,
    NotReady,
    Database(String),
}

impl From<rusqlite::Error> for AcceptError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Database(error.to_string())
    }
}

pub(crate) fn open_session(
    db: &mut Connection,
    session_id: &str,
    conversation_id: &str,
    fingerprint: &str,
    larm_key: &str,
    now: &str,
) -> rusqlite::Result<()> {
    let tx = db.transaction()?;
    tx.execute(
        "INSERT INTO conversations(id,title,task_mode,created_at,updated_at)
         VALUES (?1,'Conversation preview','conversation',?2,?2)
         ON CONFLICT(id) DO NOTHING",
        params![conversation_id, now],
    )?;
    tx.execute(
        "INSERT INTO conversation_preview_sessions
         (session_id,conversation_id,config_fingerprint,resource_state,larm_key,cleanup_state,created_at,updated_at)
         VALUES (?1,?2,?3,'preparing',?4,'none',?5,?5)
         ON CONFLICT(session_id) DO NOTHING",
        params![session_id, conversation_id, fingerprint, larm_key, now],
    )?;
    append_event(&tx, session_id, None, "opened", now, None)?;
    tx.commit()
}

pub(crate) fn set_resource_state(
    db: &mut Connection,
    session_id: &str,
    expected: &str,
    next: &str,
    connection_id: Option<&str>,
    now: &str,
) -> rusqlite::Result<bool> {
    let valid = matches!(
        (expected, next),
        (
            "preparing",
            "ready" | "failed" | "closing" | "cleanup_pending"
        ) | ("ready", "failed" | "closing")
            | ("failed", "closing" | "cleanup_pending")
            | ("closing", "closed" | "cleanup_pending")
            | ("cleanup_pending", "closing")
    );
    if !valid {
        return Ok(false);
    }
    let tx = db.transaction()?;
    let count = tx.execute(
        "UPDATE conversation_preview_sessions
         SET resource_state=?1,connection_id=COALESCE(?2,connection_id),
             cleanup_state=CASE ?1 WHEN 'closing' THEN 'pending'
               WHEN 'closed' THEN 'released' WHEN 'cleanup_pending' THEN 'failed'
               ELSE cleanup_state END,
             updated_at=?3
         WHERE session_id=?4 AND resource_state=?5",
        params![next, connection_id, now, session_id, expected],
    )?;
    if count == 1 {
        append_event(&tx, session_id, None, next, now, None)?;
    }
    tx.commit()?;
    Ok(count == 1)
}

pub(crate) fn accept_input(
    db: &mut Connection,
    input: &TextInput,
    now: &str,
) -> Result<Receipt, AcceptError> {
    validate_input(input).map_err(|_| AcceptError::InvalidInput)?;
    let tx = db.transaction()?;
    let hash: [u8; 32] = Sha256::digest(input.text.as_bytes()).into();
    let duplicate = tx
        .query_row(
            "SELECT response_id,input_hash FROM conversation_preview_responses
             WHERE session_id=?1 AND input_id=?2",
            params![input.session_id, input.input_id],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, Vec<u8>>(1)?)),
        )
        .optional()?;
    if let Some((response_id, stored_hash)) = duplicate {
        if stored_hash != hash {
            return Err(AcceptError::Conflict);
        }
        return Ok(Receipt {
            response_id,
            input_id: input.input_id.clone(),
            duplicate: true,
        });
    }
    let session = tx
        .query_row(
            "SELECT conversation_id,resource_state FROM conversation_preview_sessions WHERE session_id=?1",
            [&input.session_id],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        )
        .optional()?;
    let Some((conversation_id, resource_state)) = session else {
        return Err(AcceptError::SessionClosed);
    };
    if matches!(
        resource_state.as_str(),
        "closing" | "closed" | "cleanup_pending"
    ) {
        return Err(AcceptError::SessionClosed);
    }
    let busy: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM conversation_preview_responses
         WHERE session_id=?1 AND (frontdesk_state NOT IN
           ('completed','unsupported','failed','cancelled','interrupted')
           OR speech_state NOT IN ('played','stopped','failed','interrupted')))",
        [&input.session_id],
        |row| row.get(0),
    )?;
    if busy {
        return Err(AcceptError::Busy);
    }
    if resource_state != "ready" {
        return Err(AcceptError::NotReady);
    }
    let response_id = format!("response-{}", uuid::Uuid::new_v4().simple());
    let user_message_id = format!("message-{}", uuid::Uuid::new_v4().simple());
    let speech_id = format!("speech-{}", uuid::Uuid::new_v4().simple());
    tx.execute(
        "INSERT INTO conversation_messages(id,conversation_id,role,content,created_at)
         VALUES (?1,?2,'user',?3,?4)",
        params![user_message_id, conversation_id, input.text, now],
    )?;
    tx.execute(
        "INSERT INTO conversation_preview_responses
         (response_id,session_id,input_id,input_hash,user_message_id,frontdesk_state,
          speech_id,speech_state,created_at,updated_at)
         VALUES (?1,?2,?3,?4,?5,'awaiting_control',?6,'idle',?7,?7)",
        params![
            response_id,
            input.session_id,
            input.input_id,
            hash.as_slice(),
            user_message_id,
            speech_id,
            now
        ],
    )?;
    append_event(
        &tx,
        &input.session_id,
        Some(&response_id),
        "accepted",
        now,
        None,
    )?;
    tx.commit()?;
    Ok(Receipt {
        response_id,
        input_id: input.input_id.clone(),
        duplicate: false,
    })
}

pub(crate) fn adopt_control(
    db: &mut Connection,
    response_id: &str,
    action: Action,
    now: &str,
) -> rusqlite::Result<bool> {
    let tx = db.transaction()?;
    let (action_text, state, text) = match action {
        Action::Reply => ("reply", "streaming", ""),
        Action::Clarify => ("clarify", "streaming", ""),
        Action::Delegate => ("delegate", "unsupported", DELEGATE_EXPLANATION),
    };
    let assistant_id =
        (action == Action::Delegate).then(|| format!("message-{}", uuid::Uuid::new_v4().simple()));
    if let Some(id) = &assistant_id {
        tx.execute(
            "INSERT INTO conversation_messages(id,conversation_id,role,content,created_at)
             SELECT ?1,s.conversation_id,'assistant',?2,?3
             FROM conversation_preview_responses r
             JOIN conversation_preview_sessions s ON s.session_id=r.session_id
             WHERE r.response_id=?4 AND r.frontdesk_state='awaiting_control'",
            params![id, text, now, response_id],
        )?;
    }
    let updated = tx.execute(
        "UPDATE conversation_preview_responses
         SET action=?1,frontdesk_state=?2,public_text=?3,
             assistant_message_id=?4,version=version+1,updated_at=?5
         WHERE response_id=?6 AND frontdesk_state='awaiting_control'",
        params![action_text, state, text, assistant_id, now, response_id],
    )?;
    if updated != 1 {
        return Ok(false);
    }
    append_response_event(&tx, response_id, "control", now, Some(action_text))?;
    tx.commit()?;
    Ok(true)
}

pub(crate) fn checkpoint_clause(
    db: &mut Connection,
    response_id: &str,
    text: &str,
    clause_index: u32,
    now: &str,
) -> rusqlite::Result<bool> {
    let tx = db.transaction()?;
    let updated = tx.execute(
        "UPDATE conversation_preview_responses
         SET public_text=public_text || ?1,last_persisted_clause=?2,
             version=version+1,updated_at=?3
         WHERE response_id=?4 AND frontdesk_state='streaming'
           AND (last_persisted_clause IS NULL AND ?2=0 OR last_persisted_clause=?2-1)
           AND length(CAST(public_text AS BLOB))+length(CAST(?1 AS BLOB))<=8192",
        params![text, clause_index, now, response_id],
    )?;
    if updated == 1 {
        append_response_event(&tx, response_id, "clause_saved", now, None)?;
    }
    tx.commit()?;
    Ok(updated == 1)
}

pub(crate) fn complete_response(
    db: &mut Connection,
    response_id: &str,
    final_tail: &str,
    now: &str,
) -> rusqlite::Result<bool> {
    let tx = db.transaction()?;
    let row = tx
        .query_row(
            "SELECT s.conversation_id,r.public_text FROM conversation_preview_responses r
             JOIN conversation_preview_sessions s ON s.session_id=r.session_id
             WHERE r.response_id=?1 AND r.frontdesk_state='streaming'",
            [response_id],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        )
        .optional()?;
    let Some((conversation_id, mut body)) = row else {
        return Ok(false);
    };
    body.push_str(final_tail);
    if body.trim().is_empty() || body.len() > 8192 {
        return Ok(false);
    }
    let assistant_id = format!("message-{}", uuid::Uuid::new_v4().simple());
    tx.execute(
        "INSERT INTO conversation_messages(id,conversation_id,role,content,created_at)
         VALUES (?1,?2,'assistant',?3,?4)",
        params![assistant_id, conversation_id, body, now],
    )?;
    let updated = tx.execute(
        "UPDATE conversation_preview_responses
         SET public_text=?1,assistant_message_id=?2,frontdesk_state='completed',
             version=version+1,updated_at=?3
         WHERE response_id=?4 AND frontdesk_state='streaming'",
        params![body, assistant_id, now, response_id],
    )?;
    if updated != 1 {
        return Ok(false);
    }
    append_response_event(&tx, response_id, "completed", now, None)?;
    tx.commit()?;
    Ok(true)
}

pub(crate) fn update_speech(
    db: &mut Connection,
    response_id: &str,
    generation: u64,
    expected: &str,
    next: &str,
    played_clause: Option<u32>,
    now: &str,
) -> rusqlite::Result<bool> {
    let tx = db.transaction()?;
    let updated = tx.execute(
        "UPDATE conversation_preview_responses
         SET speech_state=?1,last_played_clause=COALESCE(?2,last_played_clause),
             version=version+1,updated_at=?3
         WHERE response_id=?4 AND speech_generation=?5 AND speech_state=?6",
        params![next, played_clause, now, response_id, generation, expected],
    )?;
    if updated == 1 {
        append_response_event(&tx, response_id, "speech_state", now, Some(next))?;
    }
    tx.commit()?;
    Ok(updated == 1)
}

pub(crate) fn fail_speech(
    db: &mut Connection,
    response_id: &str,
    code: &str,
    now: &str,
) -> rusqlite::Result<bool> {
    let tx = db.transaction()?;
    let changed = tx.execute(
        "UPDATE conversation_preview_responses
         SET speech_state='failed',speech_generation=speech_generation+1,
             failure_stage='speech',failure_code=?1,version=version+1,updated_at=?2
         WHERE response_id=?3 AND (speech_state IN
           ('idle','collecting','synthesizing','playing')
           OR (speech_state='stopping' AND ?1 IN
             ('player_stop_unconfirmed','player_join_failed','tts_request_outcome_unknown')))",
        params![code, now, response_id],
    )?;
    if changed == 1 {
        append_response_event(&tx, response_id, "speech_failed", now, Some(code))?;
    }
    tx.commit()?;
    Ok(changed == 1)
}

/// Invalidates the persisted generation after the host has already signalled
/// cancellation. `stopped` is written only after I/O exits.
pub(crate) fn request_speech_stop(
    db: &mut Connection,
    response_id: &str,
    now: &str,
) -> rusqlite::Result<Option<u64>> {
    let tx = db.transaction()?;
    let changed = tx.execute(
        "UPDATE conversation_preview_responses
         SET speech_generation=speech_generation+1,speech_state='stopping',
             version=version+1,updated_at=?1
         WHERE response_id=?2 AND speech_state NOT IN
           ('stopping','stopped','played','failed','interrupted')",
        params![now, response_id],
    )?;
    let generation = if changed == 1 {
        append_response_event(&tx, response_id, "speech_stop_requested", now, None)?;
        Some(tx.query_row(
            "SELECT speech_generation FROM conversation_preview_responses WHERE response_id=?1",
            [response_id],
            |row| row.get(0),
        )?)
    } else {
        None
    };
    tx.commit()?;
    Ok(generation)
}

pub(crate) fn cancel_response(
    db: &mut Connection,
    response_id: &str,
    now: &str,
) -> rusqlite::Result<bool> {
    let tx = db.transaction()?;
    let changed = tx.execute(
        "UPDATE conversation_preview_responses
         SET frontdesk_state='cancelled',speech_generation=speech_generation+1,
             speech_state=CASE WHEN speech_state IN
               ('stopped','played','failed','interrupted') THEN speech_state ELSE 'stopping' END,
             version=version+1,updated_at=?1
         WHERE response_id=?2 AND frontdesk_state IN ('awaiting_control','streaming')",
        params![now, response_id],
    )?;
    if changed == 1 {
        append_response_event(&tx, response_id, "cancelled", now, None)?;
    }
    tx.commit()?;
    Ok(changed == 1)
}

pub(crate) fn fail_response(
    db: &mut Connection,
    response_id: &str,
    stage: &str,
    code: &str,
    now: &str,
) -> rusqlite::Result<bool> {
    let tx = db.transaction()?;
    let changed = tx.execute(
        "UPDATE conversation_preview_responses
         SET frontdesk_state=CASE WHEN frontdesk_state='unsupported'
               THEN 'unsupported' ELSE 'failed' END,
             failure_stage=?1,failure_code=?2,
             speech_generation=speech_generation+1,
             speech_state=CASE WHEN speech_state IN
               ('stopped','played','failed','interrupted') THEN speech_state ELSE 'stopping' END,
             version=version+1,updated_at=?3
         WHERE response_id=?4 AND frontdesk_state IN
           ('awaiting_control','streaming','unsupported')
           AND (frontdesk_state!='unsupported' OR failure_code IS NULL)",
        params![stage, code, now, response_id],
    )?;
    if changed == 1 {
        append_response_event(&tx, response_id, "failed", now, Some(code))?;
    }
    tx.commit()?;
    Ok(changed == 1)
}

/// Startup never replays an unfinished response or an unconfirmed audio job.
pub(crate) fn interrupt_after_restart(db: &mut Connection, now: &str) -> rusqlite::Result<usize> {
    let tx = db.transaction()?;
    let changed = tx.execute(
        "UPDATE conversation_preview_responses
         SET frontdesk_state=CASE WHEN frontdesk_state IN
               ('awaiting_control','streaming') THEN 'interrupted' ELSE frontdesk_state END,
             speech_state=CASE WHEN speech_state IN
               ('idle','collecting','synthesizing','playing','stopping') THEN 'interrupted'
               ELSE speech_state END,
             speech_generation=speech_generation+1,version=version+1,updated_at=?1
         WHERE frontdesk_state IN ('awaiting_control','streaming') OR speech_state IN
           ('idle','collecting','synthesizing','playing','stopping')",
        [now],
    )?;
    tx.commit()?;
    Ok(changed)
}

pub(crate) struct ProviderEvent<'a> {
    pub kind: &'a str,
    pub request_id: Option<&'a str>,
    pub connection_id: Option<&'a str>,
    pub allocation_id: Option<&'a str>,
    pub detail: Option<&'a str>,
    pub now: &'a str,
}

pub(crate) fn record_provider_event(
    db: &mut Connection,
    response_id: &str,
    event: ProviderEvent<'_>,
) -> rusqlite::Result<()> {
    let tx = db.transaction()?;
    let session_id: String = tx.query_row(
        "SELECT session_id FROM conversation_preview_responses WHERE response_id=?1",
        [response_id],
        |row| row.get(0),
    )?;
    tx.execute(
        "INSERT INTO conversation_preview_events
         (session_id,event_seq,response_id,kind,wall_time,request_id,connection_id,allocation_id,detail)
         SELECT ?1,COALESCE(MAX(event_seq),0)+1,?2,?3,?4,?5,?6,?7,?8
         FROM conversation_preview_events WHERE session_id=?1",
        params![session_id, response_id, event.kind, event.now, event.request_id,
            event.connection_id, event.allocation_id, event.detail],
    )?;
    tx.commit()
}

fn append_response_event(
    db: &Connection,
    response_id: &str,
    kind: &str,
    now: &str,
    detail: Option<&str>,
) -> rusqlite::Result<()> {
    let session_id: String = db.query_row(
        "SELECT session_id FROM conversation_preview_responses WHERE response_id=?1",
        [response_id],
        |row| row.get(0),
    )?;
    append_event(db, &session_id, Some(response_id), kind, now, detail)
}

fn append_event(
    db: &Connection,
    session_id: &str,
    response_id: Option<&str>,
    kind: &str,
    now: &str,
    detail: Option<&str>,
) -> rusqlite::Result<()> {
    db.execute(
        "INSERT INTO conversation_preview_events
         (session_id,event_seq,response_id,kind,wall_time,detail)
         SELECT ?1,COALESCE(MAX(event_seq),0)+1,?2,?3,?4,?5
         FROM conversation_preview_events WHERE session_id=?1",
        params![session_id, response_id, kind, now, detail],
    )?;
    Ok(())
}
