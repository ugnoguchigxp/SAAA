//! Durable coordinator boundary for routing-root state transitions.
//!
//! Effects are returned only after their state and audit event commit.  The caller owns dispatch;
//! this module intentionally performs no provider, tool, or speech I/O.
use super::reducer::{self, Event, Phase, State, Transition};
use rusqlite::{params, Connection, OptionalExtension, Transaction};

pub(crate) fn apply(
    connection: &mut Connection,
    root_id: &str,
    event: Event,
    now_ms: i64,
) -> Result<Transition, String> {
    let transaction = connection
        .transaction()
        .map_err(|error| error.to_string())?;
    let transition = apply_in_transaction(&transaction, root_id, event, now_ms)?;
    transaction.commit().map_err(|error| error.to_string())?;
    Ok(transition)
}

/// Applies a transition within the caller's receipt transaction. This is the path used when a
/// newly accepted root is promoted from `queued` immediately before provider dispatch.
pub(crate) fn apply_in_transaction(
    transaction: &Transaction<'_>,
    root_id: &str,
    event: Event,
    now_ms: i64,
) -> Result<Transition, String> {
    let state = load_state(transaction, root_id)?;
    let transition = reducer::reduce(&state, event.clone());
    if transition.state == state {
        return Ok(transition);
    }
    let changed = transaction
        .execute(
            "UPDATE rr_roots SET phase=?1,revision=?2,cancel_requested=?3,active_slot=?4,drain_reason=?5 WHERE root_id=?6",
            params![
                phase_name(transition.state.phase),
                transition.state.revision,
                i64::from(transition.state.phase == Phase::Cancelled),
                active_slot(transition.state.phase),
                drain_reason(&event, transition.state.phase),
                root_id,
            ],
        )
        .map_err(|error| error.to_string())?;
    if changed != 1 {
        return Err("Role-routing root disappeared during transition".into());
    }
    let seq: i64 = transaction
        .query_row(
            "SELECT COALESCE(MAX(seq),0)+1 FROM rr_events WHERE root_id=?1",
            [root_id],
            |row| row.get(0),
        )
        .map_err(|error| error.to_string())?;
    transaction
        .execute(
            "INSERT INTO rr_events(root_id,seq,kind,data_json,created_at_ms) VALUES(?1,?2,?3,'{}',?4)",
            params![root_id, seq, event_name(&event), now_ms],
        )
        .map_err(|error| error.to_string())?;
    Ok(transition)
}

fn load_state(connection: &Connection, root_id: &str) -> Result<State, String> {
    connection
        .query_row(
            "SELECT phase,revision FROM rr_roots WHERE root_id=?1",
            [root_id],
            |row| {
                let phase: String = row.get(0)?;
                Ok((phase, row.get::<_, u32>(1)?))
            },
        )
        .optional()
        .map_err(|error| error.to_string())?
        .ok_or_else(|| "Role-routing root was not found".to_string())
        .and_then(|(phase, revision)| {
            let phase = parse_phase(&phase)?;
            Ok(State {
                barrier: phase == Phase::Draining,
                phase,
                revision,
            })
        })
}

fn parse_phase(value: &str) -> Result<Phase, String> {
    match value {
        "queued" => Ok(Phase::Queued),
        "responding" => Ok(Phase::Responding),
        "draining" => Ok(Phase::Draining),
        "completed" => Ok(Phase::Completed),
        "cancelled" => Ok(Phase::Cancelled),
        "failed" => Ok(Phase::Failed),
        _ => Err("Role-routing root has an invalid phase".into()),
    }
}

fn phase_name(phase: Phase) -> &'static str {
    match phase {
        Phase::Queued => "queued",
        Phase::Responding => "responding",
        Phase::Draining => "draining",
        Phase::Completed => "completed",
        Phase::Cancelled => "cancelled",
        Phase::Failed => "failed",
    }
}

fn active_slot(phase: Phase) -> Option<&'static str> {
    match phase {
        Phase::Responding => Some("reasoning"),
        Phase::Draining => Some("draining"),
        _ => None,
    }
}

fn event_name(event: &Event) -> &'static str {
    match event {
        Event::Start => "root_started",
        Event::CandidateReady { .. } => "candidate_ready",
        Event::InputBarrier => "input_barrier",
        Event::Resume => "root_resumed",
        Event::Cancel => "root_cancelled",
        Event::Fail => "root_failed",
    }
}

fn drain_reason(event: &Event, phase: Phase) -> Option<&'static str> {
    if phase != Phase::Draining {
        return None;
    }
    match event {
        Event::InputBarrier => Some("input-classification"),
        _ => Some("draining"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> Connection {
        let connection = Connection::open_in_memory().expect("database");
        connection
            .execute_batch(
                "PRAGMA foreign_keys=ON;
                 CREATE TABLE conversations(id TEXT PRIMARY KEY);
                 CREATE TABLE runtime_runs(id TEXT PRIMARY KEY);
                 CREATE TABLE conversation_messages(id TEXT PRIMARY KEY);
                 INSERT INTO conversations VALUES('c');",
            )
            .expect("base tables");
        crate::role_routing::schema::migrate(&connection).expect("schema");
        connection
            .execute(
                "INSERT INTO rr_policy_versions VALUES('p',1,'{}','d',1)",
                [],
            )
            .expect("policy");
        connection
            .execute(
                "INSERT INTO rr_roots(root_id,conversation_id,policy_id,phase,origin,presentation_mode,started_at_ms,scope_digest) VALUES('r','c','p','queued','text','visual',1,'')",
                [],
            )
            .expect("root");
        connection
    }

    #[test]
    fn rr_05_commit_before_dispatch() {
        let mut connection = fixture();
        let transition = apply(&mut connection, "r", Event::Start, 2).expect("transition");
        assert_eq!(
            transition.effects,
            vec![super::super::reducer::Effect::DispatchActor { revision: 0 }]
        );
        let phase: String = connection
            .query_row("SELECT phase FROM rr_roots WHERE root_id='r'", [], |row| {
                row.get(0)
            })
            .expect("phase");
        assert_eq!(phase, "responding");
    }

    #[test]
    fn rr_16_barrier_is_durable_before_resume() {
        let mut connection = fixture();
        apply(&mut connection, "r", Event::Start, 2).expect("start");
        apply(&mut connection, "r", Event::InputBarrier, 3).expect("barrier");
        let phase: String = connection
            .query_row("SELECT phase FROM rr_roots WHERE root_id='r'", [], |row| {
                row.get(0)
            })
            .expect("phase");
        assert_eq!(phase, "draining");
        let resumed = apply(&mut connection, "r", Event::Resume, 4).expect("resume");
        assert_eq!(resumed.state.revision, 1);
    }
}
