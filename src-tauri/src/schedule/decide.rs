use super::ledger::{Entry, FireResult};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Decision {
    Act,
    Hold,
    Ask,
    Drop(&'static str),
}

pub(crate) fn decide(entry: &Entry, meeting_hold: bool, generation_busy: bool) -> Decision {
    if entry.delegation_ref.as_ref().map(|value| value.trim().is_empty()) != Some(false) {
        return Decision::Ask;
    }
    if meeting_hold {
        return Decision::Hold;
    }
    if generation_busy {
        return Decision::Hold;
    }
    if matches!(entry.kind, super::ledger::Kind::HoldUntil) && meeting_hold {
        return Decision::Hold;
    }
    Decision::Act
}

pub(crate) fn fire_result(decision: &Decision, generation_busy: bool) -> FireResult {
    match decision {
        Decision::Act => FireResult::Started,
        Decision::Hold if generation_busy => FireResult::Deferred,
        Decision::Hold => FireResult::Deferred,
        Decision::Ask => FireResult::NoDelegation,
        Decision::Drop(_) => FireResult::SuppressedMeeting,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schedule::ledger::{Kind, Origin, Status};

    fn entry(delegation: Option<&str>) -> Entry {
        Entry {
            id: "e".into(),
            kind: Kind::TaskRun,
            subject_ref: "task:t1".into(),
            scope_ref: "scope:primary".into(),
            due_at: 1,
            window_end_at: None,
            status: Status::Firing,
            origin: Origin::Delegation,
            delegation_ref: delegation.map(str::to_string),
            revision: 1,
            supersedes: None,
            created_at: 1,
            fired_at: None,
            fire_result: None,
            payload_id: None,
        }
    }

    #[test]
    fn sl_07_ask_without_delegation_hold_on_meeting() {
        assert_eq!(decide(&entry(None), false, false), Decision::Ask);
        assert_eq!(decide(&entry(Some("")), false, false), Decision::Ask);
        assert_eq!(decide(&entry(Some("del")), true, false), Decision::Hold);
        assert_eq!(decide(&entry(Some("del")), false, true), Decision::Hold);
        assert_eq!(decide(&entry(Some("del")), false, false), Decision::Act);
    }
}
