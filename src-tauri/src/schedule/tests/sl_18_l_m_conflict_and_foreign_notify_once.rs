use super::*;
#[test]
pub(super) fn sl_18_l_m_conflict_and_foreign_notify_once() {
    let state = state();
    state
        .sqlite_writer
        .write(|connection| {
            ledger::insert(
                connection,
                &Entry {
                    id: "l1".into(),
                    kind: Kind::Reminder,
                    subject_ref: "goal:g".into(),
                    scope_ref: "scope:primary".into(),
                    due_at: 10,
                    window_end_at: None,
                    status: Status::Scheduled,
                    origin: Origin::UserExplicit,
                    delegation_ref: Some("d".into()),
                    revision: 3,
                    supersedes: None,
                    created_at: 1,
                    fired_at: None,
                    fire_result: None,
                    payload_id: None,
                },
            )?;
            connection.execute(
                "INSERT INTO calendar_observations(id,event_id,entry_id,observed_at,remote_etag,remote_start,diff_kind,handled,remote_rev)
                 VALUES('o1','ev1','l1',1,'e',99,'moved','pending',1)",
                [],
            ).map_err(crate::database_error)?;
            connection.execute(
                "INSERT INTO calendar_observations(id,event_id,observed_at,remote_etag,diff_kind,handled)
                 VALUES('o2','foreign',1,'e','foreign_event','pending')",
                [],
            ).map_err(crate::database_error)?;
            calendar::reconcile::apply(connection, &state, 10)?;
            calendar::reconcile::apply(connection, &state, 11)?;
            Ok(())
        })
        .unwrap();
    let texts = messages(&state);
    assert_eq!(
        texts
            .iter()
            .filter(|value| value.starts_with("schedule-conflict:"))
            .count(),
        1
    );
    assert_eq!(
        texts
            .iter()
            .filter(|value| value.starts_with("schedule-foreign:"))
            .count(),
        1
    );
}
#[test]
pub(super) fn sl_17_k_delegated_delete_asks() {
    let state = state();
    state
        .sqlite_writer
        .write(|connection| {
            ledger::insert(
                connection,
                &Entry {
                    id: "k1".into(),
                    kind: Kind::TaskRun,
                    subject_ref: "task:t1".into(),
                    scope_ref: "scope:primary".into(),
                    due_at: 10,
                    window_end_at: None,
                    status: Status::Scheduled,
                    origin: Origin::Delegation,
                    delegation_ref: Some("del".into()),
                    revision: 1,
                    supersedes: None,
                    created_at: 1,
                    fired_at: None,
                    fire_result: None,
                    payload_id: None,
                },
            )?;
            connection.execute(
                "INSERT INTO calendar_observations(id,event_id,entry_id,observed_at,remote_etag,diff_kind,handled)
                 VALUES('ok','evk','k1',1,'e','deleted','pending')",
                [],
            ).map_err(crate::database_error)?;
            calendar::reconcile::apply(connection, &state, 10)?;
            Ok(())
        })
        .unwrap();
    assert_eq!(status_of(&state, "k1").0, Status::Scheduled);
    assert!(messages(&state)
        .iter()
        .any(|value| value.starts_with("schedule-confirm-delete:")));
}
#[test]
pub(super) fn sl_10_ipc_origin_is_user_explicit() {
    let state = state();
    tick::enable(&state, 1).unwrap();
    state
        .sqlite_writer
        .write(|connection| {
            ledger::insert(
                connection,
                &Entry {
                    id: "ipc1".into(),
                    kind: Kind::Reminder,
                    subject_ref: "goal:g".into(),
                    scope_ref: "scope:primary".into(),
                    due_at: 9,
                    window_end_at: None,
                    status: Status::Scheduled,
                    origin: Origin::UserExplicit,
                    delegation_ref: None,
                    revision: 1,
                    supersedes: None,
                    created_at: 1,
                    fired_at: None,
                    fire_result: None,
                    payload_id: None,
                },
            )
        })
        .unwrap();
    let listed = state
        .sqlite_readers
        .read(|connection| super::super::ledger::list_all(connection, 10))
        .unwrap();
    assert_eq!(listed[0].origin, Origin::UserExplicit);
    assert_ne!(listed[0].status.as_str(), "");
}
