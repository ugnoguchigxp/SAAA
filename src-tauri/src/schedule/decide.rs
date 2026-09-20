use super::ledger::{Entry, FireResult};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Decision {
    Act,
    Hold,
    Defer,
    Ask,
}

pub(crate) fn decide(entry: &Entry, situation_hold: bool, generation_busy: bool) -> Decision {
    if !entry.may_act() {
        return Decision::Ask;
    }
    if situation_hold {
        return Decision::Hold;
    }
    if generation_busy {
        return Decision::Defer;
    }
    Decision::Act
}

pub(crate) fn fire_result(decision: &Decision) -> FireResult {
    match decision {
        Decision::Act => FireResult::Started,
        Decision::Hold | Decision::Defer => FireResult::Deferred,
        Decision::Ask => FireResult::NoDelegation,
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
    fn sl_07_ask_without_delegation_hold_on_situation() {
        assert_eq!(decide(&entry(None), false, false), Decision::Ask);
        assert_eq!(decide(&entry(Some("")), false, false), Decision::Ask);
        assert_eq!(decide(&entry(Some("del")), true, false), Decision::Hold);
        assert_eq!(decide(&entry(Some("del")), false, true), Decision::Defer);
        assert_eq!(decide(&entry(Some("del")), false, false), Decision::Act);
    }
}
