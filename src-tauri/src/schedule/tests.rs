use super::ledger::{allowed_transition, Entry, FireResult, Kind, LedgerError, Origin, Status};
use std::str::FromStr;

fn scheduled(delegation_ref: Option<&str>) -> Entry {
    Entry {
        id: "entry-1".into(),
        kind: Kind::Reminder,
        subject_ref: "goal:demo".into(),
        scope_ref: "user:primary".into(),
        due_at: 1,
        window_end_at: Some(10),
        status: Status::Scheduled,
        origin: Origin::UserExplicit,
        delegation_ref: delegation_ref.map(str::to_string),
        revision: 1,
        supersedes: None,
        created_at: 0,
        fired_at: None,
        fire_result: None,
        payload_id: None,
    }
}

#[test]
fn sl_01_scheduled_to_firing_to_fired_is_allowed() {
    let firing = scheduled(Some("delegation-1"))
        .transition(Status::Firing)
        .expect("claim");
    let fired = firing.transition(Status::Fired).expect("complete");
    assert!(allowed_transition(Status::Scheduled, Status::Firing));
    assert!(allowed_transition(Status::Firing, Status::Fired));
    assert_eq!(fired.status, Status::Fired);
}

#[test]
fn sl_01_fired_cannot_return_to_scheduled() {
    let fired = scheduled(Some("delegation-1"))
        .transition(Status::Firing)
        .and_then(|entry| entry.transition(Status::Fired))
        .expect("fire");
    let error = fired.transition(Status::Scheduled).expect_err("reverse");
    assert_eq!(
        error,
        LedgerError::ForbiddenTransition {
            from: Status::Fired,
            to: Status::Scheduled,
        }
    );
    assert!(!allowed_transition(Status::Fired, Status::Firing));
    assert!(!allowed_transition(Status::Missed, Status::Scheduled));
}

#[test]
fn sl_01_withdrawn_and_superseded_are_terminal() {
    let withdrawn = scheduled(None)
        .transition(Status::Withdrawn)
        .expect("withdraw");
    assert_eq!(
        withdrawn.transition(Status::Scheduled).expect_err("closed"),
        LedgerError::ForbiddenTransition {
            from: Status::Withdrawn,
            to: Status::Scheduled,
        }
    );
    let superseded = scheduled(None)
        .transition_superseded("entry-2")
        .expect("replace");
    assert_eq!(
        superseded.transition(Status::Firing).expect_err("closed"),
        LedgerError::ForbiddenTransition {
            from: Status::Superseded,
            to: Status::Firing,
        }
    );
}

#[test]
fn sl_01_superseded_requires_supersedes() {
    let entry = scheduled(None);
    assert_eq!(
        entry.transition(Status::Superseded).expect_err("link"),
        LedgerError::SupersedesRequired
    );
    assert_eq!(
        entry.transition_superseded("").expect_err("empty"),
        LedgerError::SupersedesRequired
    );
    let mut invalid = scheduled(None);
    invalid.status = Status::Superseded;
    invalid.supersedes = None;
    assert_eq!(
        invalid.validate().expect_err("missing"),
        LedgerError::SupersedesRequired
    );
    let replaced = entry.transition_superseded("entry-2").expect("ok");
    assert_eq!(replaced.status, Status::Superseded);
    assert_eq!(replaced.supersedes.as_deref(), Some("entry-2"));
}

#[test]
fn sl_01_null_delegation_cannot_act() {
    assert!(!scheduled(None).may_act());
    assert!(scheduled(Some("delegation-1")).may_act());
    assert_eq!(FireResult::NoDelegation.as_str(), "no_delegation");
}

#[test]
fn sl_01_kind_status_origin_roundtrip() {
    for kind in [
        Kind::TaskRun,
        Kind::Reminder,
        Kind::CheckIn,
        Kind::Digest,
        Kind::HoldUntil,
    ] {
        assert_eq!(Kind::from_str(kind.as_str()), Ok(kind));
    }
    for status in [
        Status::Scheduled,
        Status::Firing,
        Status::Fired,
        Status::Missed,
        Status::Withdrawn,
        Status::Superseded,
    ] {
        assert_eq!(Status::from_str(status.as_str()), Ok(status));
    }
    for origin in [
        Origin::UserExplicit,
        Origin::Delegation,
        Origin::PlannerCandidate,
        Origin::UserCalendarEdit,
    ] {
        assert_eq!(Origin::from_str(origin.as_str()), Ok(origin));
    }
    let crashed = FireResult::from_str("error:crashed").expect("code");
    assert_eq!(crashed, FireResult::Error("crashed".into()));
    assert_eq!(crashed.as_str(), "error:crashed");
    assert!(FireResult::from_str("error:").is_err());
    assert!(allowed_transition(Status::Scheduled, Status::Missed));
    assert!(allowed_transition(Status::Firing, Status::Missed));
}
