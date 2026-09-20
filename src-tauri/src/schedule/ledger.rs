use std::fmt::{Display, Formatter};
use std::str::FromStr;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    TaskRun,
    Reminder,
    CheckIn,
    Digest,
    HoldUntil,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Status {
    Scheduled,
    Firing,
    Fired,
    Missed,
    Withdrawn,
    Superseded,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Origin {
    UserExplicit,
    Delegation,
    PlannerCandidate,
    UserCalendarEdit,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum FireResult {
    Started,
    Deferred,
    SuppressedMeeting,
    NoDelegation,
    Error(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum LedgerError {
    ForbiddenTransition { from: Status, to: Status },
    SupersedesRequired,
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
}

impl Display for Kind {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl Display for Status {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl Display for Origin {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl Display for FireResult {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.as_str())
    }
}

impl FromStr for Kind {
    type Err = ();

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "task_run" => Ok(Self::TaskRun),
            "reminder" => Ok(Self::Reminder),
            "check_in" => Ok(Self::CheckIn),
            "digest" => Ok(Self::Digest),
            "hold_until" => Ok(Self::HoldUntil),
            _ => Err(()),
        }
    }
}

impl FromStr for Status {
    type Err = ();

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "scheduled" => Ok(Self::Scheduled),
            "firing" => Ok(Self::Firing),
            "fired" => Ok(Self::Fired),
            "missed" => Ok(Self::Missed),
            "withdrawn" => Ok(Self::Withdrawn),
            "superseded" => Ok(Self::Superseded),
            _ => Err(()),
        }
    }
}

impl FromStr for Origin {
    type Err = ();

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "user_explicit" => Ok(Self::UserExplicit),
            "delegation" => Ok(Self::Delegation),
            "planner_candidate" => Ok(Self::PlannerCandidate),
            "user_calendar_edit" => Ok(Self::UserCalendarEdit),
            _ => Err(()),
        }
    }
}

impl FromStr for FireResult {
    type Err = ();

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "started" => Ok(Self::Started),
            "deferred" => Ok(Self::Deferred),
            "suppressed_meeting" => Ok(Self::SuppressedMeeting),
            "no_delegation" => Ok(Self::NoDelegation),
            other => other
                .strip_prefix("error:")
                .filter(|code| !code.is_empty() && !code.contains(':'))
                .map(|code| Self::Error(code.to_string()))
                .ok_or(()),
        }
    }
}

pub(crate) fn allowed_transition(from: Status, to: Status) -> bool {
    matches!(
        (from, to),
        (
            Status::Scheduled,
            Status::Firing | Status::Missed | Status::Withdrawn | Status::Superseded
        ) | (
            Status::Firing,
            Status::Fired | Status::Missed | Status::Withdrawn | Status::Superseded
        ) | (Status::Fired, Status::Withdrawn | Status::Superseded)
            | (Status::Missed, Status::Withdrawn | Status::Superseded)
    )
}

impl Entry {
    pub(crate) fn validate(&self) -> Result<(), LedgerError> {
        if self.status == Status::Superseded && self.supersedes.is_none() {
            return Err(LedgerError::SupersedesRequired);
        }
        Ok(())
    }

    pub(crate) fn may_act(&self) -> bool {
        self.delegation_ref.is_some()
    }

    pub(crate) fn transition(&self, next: Status) -> Result<Self, LedgerError> {
        if next == Status::Superseded {
            return Err(LedgerError::SupersedesRequired);
        }
        if !allowed_transition(self.status, next) {
            return Err(LedgerError::ForbiddenTransition {
                from: self.status,
                to: next,
            });
        }
        let mut entry = self.clone();
        entry.status = next;
        entry.validate()?;
        Ok(entry)
    }

    pub(crate) fn transition_superseded(&self, replacement_id: &str) -> Result<Self, LedgerError> {
        if replacement_id.is_empty() {
            return Err(LedgerError::SupersedesRequired);
        }
        if !allowed_transition(self.status, Status::Superseded) {
            return Err(LedgerError::ForbiddenTransition {
                from: self.status,
                to: Status::Superseded,
            });
        }
        let mut entry = self.clone();
        entry.status = Status::Superseded;
        if entry.supersedes.is_none() {
            entry.supersedes = Some(replacement_id.to_string());
        }
        entry.validate()?;
        Ok(entry)
    }
}
