use crate::database_error;
use rusqlite::{params, Connection, OptionalExtension};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    TaskRun,
    Reminder,
    CheckIn,
    Digest,
    HoldUntil,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Status {
    Scheduled,
    Firing,
    Fired,
    Missed,
    Withdrawn,
    Superseded,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Origin {
    UserExplicit,
    Delegation,
    PlannerCandidate,
    UserCalendarEdit,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum FireResult {
    Started,
    Deferred,
    SuppressedMeeting,
    NoDelegation,
    Error(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Entry {
    pub(crate) id: String,
    pub(crate) kind: Kind,
    pub(crate) subject_ref: String,
    pub(crate) scope_ref: String,
    pub(crate) due_at: i64,
    pub(crate) window_end_at: Option<i64>,
    pub(crate) status: Status,
    pub(crate) origin: Origin,
    pub(crate) delegation_ref: Option<String>,
    pub(crate) revision: i64,
    pub(crate) supersedes: Option<String>,
    pub(crate) created_at: i64,
    pub(crate) fired_at: Option<i64>,
    pub(crate) fire_result: Option<FireResult>,
    pub(crate) payload_id: Option<String>,
}

pub(crate) fn can_transition(from: Status, to: Status) -> bool {
    if from == to {
        return false;
    }
    if matches!(to, Status::Withdrawn | Status::Superseded) {
        return !matches!(from, Status::Withdrawn | Status::Superseded);
    }
    matches!(
        (from, to),
        (Status::Scheduled, Status::Firing)
            | (Status::Firing, Status::Fired)
            | (Status::Scheduled, Status::Missed)
            | (Status::Firing, Status::Missed)
    )
}

impl Kind {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::TaskRun => "task_run",
            Self::Reminder => "reminder",
            Self::CheckIn => "check_in",
            Self::Digest => "digest",
            Self::HoldUntil => "hold_until",
        }
    }
    fn parse(value: &str) -> Result<Self, String> {
        match value {
            "task_run" => Ok(Self::TaskRun),
            "reminder" => Ok(Self::Reminder),
            "check_in" => Ok(Self::CheckIn),
            "digest" => Ok(Self::Digest),
            "hold_until" => Ok(Self::HoldUntil),
            _ => Err("invalid-schedule-kind".into()),
        }
    }
}

impl Status {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Scheduled => "scheduled",
            Self::Firing => "firing",
            Self::Fired => "fired",
            Self::Missed => "missed",
            Self::Withdrawn => "withdrawn",
            Self::Superseded => "superseded",
        }
    }
    fn parse(value: &str) -> Result<Self, String> {
        match value {
            "scheduled" => Ok(Self::Scheduled),
            "firing" => Ok(Self::Firing),
            "fired" => Ok(Self::Fired),
            "missed" => Ok(Self::Missed),
            "withdrawn" => Ok(Self::Withdrawn),
            "superseded" => Ok(Self::Superseded),
            _ => Err("invalid-schedule-status".into()),
        }
    }
}

impl Origin {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::UserExplicit => "user_explicit",
            Self::Delegation => "delegation",
            Self::PlannerCandidate => "planner_candidate",
            Self::UserCalendarEdit => "user_calendar_edit",
        }
    }
    fn parse(value: &str) -> Result<Self, String> {
        match value {
            "user_explicit" => Ok(Self::UserExplicit),
            "delegation" => Ok(Self::Delegation),
            "planner_candidate" => Ok(Self::PlannerCandidate),
            "user_calendar_edit" => Ok(Self::UserCalendarEdit),
            _ => Err("invalid-schedule-origin".into()),
        }
    }
}

impl FireResult {
    pub(crate) fn as_str(&self) -> String {
        match self {
            Self::Started => "started".into(),
            Self::Deferred => "deferred".into(),
            Self::SuppressedMeeting => "suppressed_meeting".into(),
            Self::NoDelegation => "no_delegation".into(),
            Self::Error(code) => format!("error:{code}"),
        }
    }
    fn parse(value: &str) -> Result<Self, String> {
        match value {
            "started" => Ok(Self::Started),
            "deferred" => Ok(Self::Deferred),
            "suppressed_meeting" => Ok(Self::SuppressedMeeting),
            "no_delegation" => Ok(Self::NoDelegation),
            other if other.starts_with("error:") => Ok(Self::Error(other[6..].into())),
            _ => Err("invalid-fire-result".into()),
        }
    }
}

pub(crate) fn insert_payload(
    connection: &Connection,
    id: &str,
    body: Option<&str>,
    classification: &str,
) -> Result<(), String> {
    connection
        .execute(
            "INSERT INTO schedule_payloads(id, body, classification) VALUES(?1,?2,?3)",
            params![id, body, classification],
        )
        .map_err(database_error)?;
    Ok(())
}

pub(crate) fn insert(connection: &Connection, entry: &Entry) -> Result<(), String> {
    if entry.supersedes.is_none() && entry.status == Status::Superseded {
        return Err("superseded-requires-supersedes".into());
    }
    connection
        .execute(
            "INSERT INTO schedule_entries(
               id, kind, subject_ref, scope_ref, due_at, window_end_at, status, origin,
               delegation_ref, revision, supersedes, created_at, fired_at, fire_result, payload_id
             ) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15)",
            params![
                entry.id,
                entry.kind.as_str(),
                entry.subject_ref,
                entry.scope_ref,
                entry.due_at,
                entry.window_end_at,
                entry.status.as_str(),
                entry.origin.as_str(),
                entry.delegation_ref,
                entry.revision,
                entry.supersedes,
                entry.created_at,
                entry.fired_at,
                entry.fire_result.as_ref().map(|value| value.as_str()),
                entry.payload_id
            ],
        )
        .map_err(database_error)?;
    Ok(())
}

pub(crate) fn cas_status(
    connection: &Connection,
    id: &str,
    revision: i64,
    from: Status,
    to: Status,
    fire_result: Option<&FireResult>,
    fired_at: Option<i64>,
) -> Result<bool, String> {
    if !can_transition(from, to) {
        return Err("illegal-schedule-transition".into());
    }
    let changed = connection
        .execute(
            "UPDATE schedule_entries
             SET status=?1, fire_result=COALESCE(?2, fire_result), fired_at=COALESCE(?3, fired_at)
             WHERE id=?4 AND revision=?5 AND status=?6",
            params![
                to.as_str(),
                fire_result.map(|value| value.as_str()),
                fired_at,
                id,
                revision,
                from.as_str()
            ],
        )
        .map_err(database_error)?;
    Ok(changed == 1)
}

pub(crate) fn supersede(
    connection: &Connection,
    old: &Entry,
    new_entry: &Entry,
) -> Result<(), String> {
    if new_entry.supersedes.as_deref() != Some(old.id.as_str()) {
        return Err("supersede-link-required".into());
    }
    let tx = connection.unchecked_transaction().map_err(database_error)?;
    let marked = tx
        .execute(
            "UPDATE schedule_entries SET status='superseded' WHERE id=?1 AND revision=?2 AND status=?3",
            params![old.id, old.revision, old.status.as_str()],
        )
        .map_err(database_error)?;
    if marked != 1 {
        return Err("supersede-cas-failed".into());
    }
    insert(&tx, new_entry)?;
    tx.commit().map_err(database_error)?;
    Ok(())
}

pub(crate) fn withdraw(connection: &Connection, id: &str, revision: i64) -> Result<bool, String> {
    let current = get(connection, id)?.ok_or_else(|| "schedule-entry-missing".to_string())?;
    if !can_transition(current.status, Status::Withdrawn) {
        return Err("illegal-schedule-transition".into());
    }
    cas_status(
        connection,
        id,
        revision,
        current.status,
        Status::Withdrawn,
        None,
        None,
    )
}

pub(crate) fn get(connection: &Connection, id: &str) -> Result<Option<Entry>, String> {
    connection
        .query_row(
            "SELECT id, kind, subject_ref, scope_ref, due_at, window_end_at, status, origin,
                    delegation_ref, revision, supersedes, created_at, fired_at, fire_result, payload_id
             FROM schedule_entries WHERE id=?1",
            [id],
            row_entry,
        )
        .optional()
        .map_err(database_error)
}

pub(crate) fn due(
    connection: &Connection,
    now: i64,
    limit: i64,
) -> Result<Vec<Entry>, String> {
    let mut statement = connection
        .prepare(
            "SELECT id, kind, subject_ref, scope_ref, due_at, window_end_at, status, origin,
                    delegation_ref, revision, supersedes, created_at, fired_at, fire_result, payload_id
             FROM schedule_entries
             WHERE status='scheduled' AND due_at<=?1
               AND NOT EXISTS (
                 SELECT 1 FROM calendar_observations o
                 WHERE o.entry_id=schedule_entries.id AND o.handled='asked'
               )
             ORDER BY due_at ASC
             LIMIT ?2",
        )
        .map_err(database_error)?;
    let rows = statement
        .query_map(params![now, limit], row_entry)
        .map_err(database_error)?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(database_error)
}

pub(crate) fn list_all(connection: &Connection, limit: i64) -> Result<Vec<Entry>, String> {
    let mut statement = connection
        .prepare(
            "SELECT id, kind, subject_ref, scope_ref, due_at, window_end_at, status, origin,
                    delegation_ref, revision, supersedes, created_at, fired_at, fire_result, payload_id
             FROM schedule_entries ORDER BY due_at DESC LIMIT ?1",
        )
        .map_err(database_error)?;
    let rows = statement
        .query_map([limit], row_entry)
        .map_err(database_error)?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(database_error)
}

pub(crate) fn lineage(connection: &Connection, id: &str) -> Result<Vec<String>, String> {
    let mut ids = Vec::new();
    let mut current = Some(id.to_string());
    while let Some(entry_id) = current.take() {
        ids.push(entry_id.clone());
        current = connection
            .query_row(
                "SELECT id FROM schedule_entries WHERE supersedes=?1",
                [&entry_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(database_error)?;
    }
    Ok(ids)
}

pub(crate) fn mark_missed(connection: &Connection, now: i64) -> Result<usize, String> {
    connection
        .execute(
            "UPDATE schedule_entries SET status='missed', fire_result='error:window'
             WHERE status='scheduled' AND window_end_at IS NOT NULL AND window_end_at<?1",
            [now],
        )
        .map_err(database_error)
}

pub(crate) fn close_firing(connection: &Connection, code: &str, now: i64) -> Result<usize, String> {
    connection
        .execute(
            "UPDATE schedule_entries SET status='fired', fire_result=?1, fired_at=?2
             WHERE status='firing'",
            params![format!("error:{code}"), now],
        )
        .map_err(database_error)
}

fn row_entry(row: &rusqlite::Row<'_>) -> rusqlite::Result<Entry> {
    let fire: Option<String> = row.get(13)?;
    Ok(Entry {
        id: row.get(0)?,
        kind: Kind::parse(&row.get::<_, String>(1)?).map_err(|error| {
            rusqlite::Error::FromSqlConversionFailure(1, rusqlite::types::Type::Text, error.into())
        })?,
        subject_ref: row.get(2)?,
        scope_ref: row.get(3)?,
        due_at: row.get(4)?,
        window_end_at: row.get(5)?,
        status: Status::parse(&row.get::<_, String>(6)?).map_err(|error| {
            rusqlite::Error::FromSqlConversionFailure(6, rusqlite::types::Type::Text, error.into())
        })?,
        origin: Origin::parse(&row.get::<_, String>(7)?).map_err(|error| {
            rusqlite::Error::FromSqlConversionFailure(7, rusqlite::types::Type::Text, error.into())
        })?,
        delegation_ref: row.get(8)?,
        revision: row.get(9)?,
        supersedes: row.get(10)?,
        created_at: row.get(11)?,
        fired_at: row.get(12)?,
        fire_result: fire
            .as_deref()
            .map(FireResult::parse)
            .transpose()
            .map_err(|error| {
                rusqlite::Error::FromSqlConversionFailure(
                    13,
                    rusqlite::types::Type::Text,
                    error.into(),
                )
            })?,
        payload_id: row.get(14)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schedule::schema::migrate;

    fn db() -> Connection {
        let connection = Connection::open_in_memory().unwrap();
        migrate(&connection).unwrap();
        connection
    }

    fn sample(id: &str, status: Status) -> Entry {
        Entry {
            id: id.into(),
            kind: Kind::Reminder,
            subject_ref: "goal:g1".into(),
            scope_ref: "scope:primary".into(),
            due_at: 10,
            window_end_at: None,
            status,
            origin: Origin::UserExplicit,
            delegation_ref: Some("del-1".into()),
            revision: 1,
            supersedes: None,
            created_at: 1,
            fired_at: None,
            fire_result: None,
            payload_id: None,
        }
    }

    #[test]
    fn sl_01_rejects_reverse_and_requires_supersedes() {
        assert!(!can_transition(Status::Fired, Status::Scheduled));
        assert!(!can_transition(Status::Withdrawn, Status::Scheduled));
        assert!(can_transition(Status::Scheduled, Status::Firing));
        assert!(can_transition(Status::Scheduled, Status::Withdrawn));
        let connection = db();
        let mut replacement = sample("e1", Status::Scheduled);
        replacement.supersedes = None;
        insert(&connection, &replacement).unwrap();
        let mut bad = sample("e1b", Status::Superseded);
        assert!(insert(&connection, &bad).is_err());
        bad.supersedes = Some("e0".into());
        insert(&connection, &bad).expect("linked superseded");
        assert!(sample("e1", Status::Scheduled).delegation_ref.is_some());
        let ask = sample("e2", Status::Scheduled);
        assert!(ask.delegation_ref.is_some());
        let mut none = sample("e3", Status::Scheduled);
        none.delegation_ref = None;
        insert(&connection, &none).unwrap();
        assert!(get(&connection, "e3").unwrap().unwrap().delegation_ref.is_none());
    }

    #[test]
    fn sl_03_cas_zero_rows_and_atomic_supersede() {
        let connection = db();
        insert(&connection, &sample("old", Status::Scheduled)).unwrap();
        assert!(!cas_status(
            &connection,
            "old",
            2,
            Status::Scheduled,
            Status::Firing,
            None,
            None
        )
        .unwrap());
        let old = get(&connection, "old").unwrap().unwrap();
        let mut next = sample("new", Status::Scheduled);
        next.supersedes = Some("old".into());
        next.revision = 2;
        supersede(&connection, &old, &next).unwrap();
        assert_eq!(
            get(&connection, "old").unwrap().unwrap().status,
            Status::Superseded
        );
        assert_eq!(
            get(&connection, "new").unwrap().unwrap().supersedes.as_deref(),
            Some("old")
        );
    }

    #[test]
    fn sl_04_due_uses_index_order() {
        let connection = db();
        for index in 0..40 {
            let mut entry = sample(&format!("e{index}"), Status::Scheduled);
            entry.due_at = 100 + index;
            insert(&connection, &entry).unwrap();
        }
        let due = due(&connection, 131, 32).unwrap();
        assert_eq!(due.len(), 32);
        assert!(due.windows(2).all(|pair| pair[0].due_at <= pair[1].due_at));
        assert_eq!(lineage(&connection, "e0").unwrap(), vec!["e0".to_string()]);
    }
}
