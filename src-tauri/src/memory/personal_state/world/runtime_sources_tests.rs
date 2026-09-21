#![cfg(test)]
use super::runtime_test_support::{Fixture, RUN_ID};
use saaa_personal_state_core::world::{frame_sources::*, runtime_frame::FrameValidity};
use std::sync::Arc;

#[test]
fn wr_t06_sources_reach_frame_and_schedule_addition_invalidates_it() {
    let f = Fixture::new(&[]);
    let situation =
        Arc::new(crate::situation::SituationRuntime::new(Default::default(), None).unwrap());
    let service = f.service().with_sources(situation);
    let prepared = service
        .prepare_frame(f.request(f.access(), vec![], None))
        .unwrap();
    assert!(prepared
        .frame()
        .sources
        .iter()
        .any(|g| g.kind == WorldSourceKind::Situation));
    let view = prepared
        .frame()
        .sources
        .iter()
        .find(|g| g.kind == WorldSourceKind::Situation)
        .unwrap();
    assert!(view.entries[0].availability != WorldSourceAvailability::Available);
    assert_eq!(
        service.revalidate_frame(&prepared).unwrap(),
        FrameValidity::Current
    );
    // A new row changes the selected collection even though no prior row revision changed.
    f.writer.write(|c| {
        c.execute("INSERT INTO schedule_entries(id,scope_ref,revision,status,due_at,kind,subject_ref,origin,created_at) VALUES('deadline',?1,1,'scheduled',1200,'reminder','fixture','user_explicit',1000)",[&f.project]).map_err(crate::database_error)?;
        Ok(())
    }).unwrap();
    assert_eq!(
        service.revalidate_frame(&prepared).unwrap(),
        FrameValidity::Changed
    );
}

#[test]
fn wr_t02_user_only_frame_is_authorized_without_a_project() {
    let f = Fixture::new(&[]);
    let key = format!("user:{}", f.principal);
    f.writer
        .write(|c| {
            c.execute(
                "DELETE FROM runtime_run_scopes WHERE run_id=?1 AND scope_key NOT LIKE 'user:%'",
                [RUN_ID],
            )
            .map_err(crate::database_error)?;
            c.execute(
                "UPDATE runtime_run_scopes SET relation='focus' WHERE run_id=?1",
                [RUN_ID],
            )
            .map_err(crate::database_error)?;
            Ok(())
        })
        .unwrap();
    let situation =
        Arc::new(crate::situation::SituationRuntime::new(Default::default(), None).unwrap());
    let service = f.service().with_sources(situation);
    let mut access = f.access();
    access.task_request = Some(&key);
    let mut request = f.request(access, vec![], None);
    request.project_scope = &key;
    let prepared = service.prepare_frame(request).unwrap();
    assert!(prepared.frame().scope.focus_scope_key.is_none());
    assert!(prepared.frame().project_scope.is_empty());
    assert_eq!(prepared.frame().scope.allowed_scope_keys, vec![key]);
    assert_eq!(
        service.revalidate_frame(&prepared).unwrap(),
        FrameValidity::Current
    );
}

#[test]
fn wr_t04_delegation_uses_workspace_ownership_and_detects_withdrawal() {
    let f = Fixture::new(&[("resource", "workspace1")]);
    f.writer.write(|c| {
        c.execute_batch("INSERT INTO steward_goals(id,conversation_id,origin,success_condition,status,created_at) VALUES('goal','conversation_primary','user_explicit','fixture','active','1');
        INSERT INTO steward_delegations(id,goal_id,conversation_id,workspace_id,ops,budget_runs,budget_ms,notify,status,created_at) VALUES('delegation','goal','conversation_primary','workspace1','read',1,1000,'silent','active','1');
        INSERT INTO steward_tasks(id,delegation_id,conversation_id,trigger_kind,source_id,dedupe_key,loop_state,created_at,updated_at) VALUES('delegated-task','delegation','conversation_primary','start','msg1','fixture','running','1','1');").map_err(crate::database_error)?;
        Ok(())
    }).unwrap();
    let service = f.service().with_sources(Arc::new(
        crate::situation::SituationRuntime::new(Default::default(), None).unwrap(),
    ));
    let frame = service
        .prepare_frame(f.request(f.access(), vec![], None))
        .unwrap();
    let group = frame
        .frame()
        .sources
        .iter()
        .find(|g| g.kind == WorldSourceKind::Delegation)
        .unwrap();
    assert_eq!(group.entries.len(), 1);
    assert_eq!(group.entries[0].owner_scope_key, "resource:workspace1");
    f.writer
        .write(|c| {
            c.execute(
                "UPDATE steward_delegations SET status='withdrawn' WHERE id='delegation'",
                [],
            )
            .map_err(crate::database_error)?;
            Ok(())
        })
        .unwrap();
    assert_eq!(
        service.revalidate_frame(&frame).unwrap(),
        FrameValidity::Changed
    );
    f.writer.write(|c|{c.execute("UPDATE steward_delegations SET workspace_id='other-workspace' WHERE id='delegation'",[]).map_err(crate::database_error)?;Ok(())}).unwrap();
    let isolated = service
        .prepare_frame(f.request(f.access(), vec![], None))
        .unwrap();
    assert!(isolated
        .frame()
        .sources
        .iter()
        .find(|g| g.kind == WorldSourceKind::Delegation)
        .unwrap()
        .entries
        .is_empty());
}

#[test]
fn wr_t07_result_acceptance_expires_only_on_changed_dependencies() {
    let f = Fixture::new(&[]);
    let service = f.service().with_sources(Arc::new(
        crate::situation::SituationRuntime::new(Default::default(), None).unwrap(),
    ));
    let old = service
        .prepare_frame(f.request(f.access(), vec![], None))
        .unwrap();
    assert_eq!(
        service.validate_result(&old).unwrap(),
        FrameValidity::Current
    );
    f.writer.write(|c|{c.execute("INSERT INTO schedule_entries(id,scope_ref,revision,status,due_at,kind,subject_ref,origin,created_at) VALUES('new',?1,1,'scheduled',1200,'reminder','fixture','user_explicit',1000)",[&f.project]).map_err(crate::database_error)?;Ok(())}).unwrap();
    assert_eq!(
        service.validate_result(&old).unwrap(),
        FrameValidity::Changed
    );
    let fresh = service.refresh(&old).unwrap();
    assert_eq!(
        service.validate_result(&fresh).unwrap(),
        FrameValidity::Current
    );
}

#[test]
fn wr_t11_refresh_replaces_actual_block_and_receipt_candidate() {
    use crate::runtime::context::world::{
        render::render_world_frame_explicit,
        turn::{WorldBlocks, WorldLive},
    };
    let f = Fixture::new(&[]);
    let service = Arc::new(f.service().with_sources(Arc::new(
        crate::situation::SituationRuntime::new(Default::default(), None).unwrap(),
    )));
    let old = service
        .prepare_frame(f.request(f.access(), vec![], None))
        .unwrap();
    let content = render_world_frame_explicit(old.frame()).unwrap();
    let world = WorldLive::live(
        service,
        old,
        Some(WorldBlocks {
            with_world: format!("personal\n{content}"),
            without_world: Some("personal".into()),
        }),
    );
    f.writer.write(|c|{c.execute("INSERT INTO schedule_entries(id,scope_ref,revision,status,due_at,kind,subject_ref,origin,created_at) VALUES('new',?1,1,'scheduled',1200,'reminder','fixture','user_explicit',1000)",[&f.project]).map_err(crate::database_error)?;Ok(())}).unwrap();
    assert!(!world.revalidate_current());
    let (old, new) = world.refresh_blocks().unwrap().unwrap();
    assert!(!old.with_world.contains("entry_id\":\"new"));
    assert!(new.with_world.contains("new"));
    assert!(world.current_candidate().unwrap().content.contains("new"));
    assert!(world.revalidate_current());
    assert!(world.refresh_blocks().unwrap().is_some());
}

#[test]
fn wr_t05_deadlines_are_globally_sorted_and_cancelled_rows_disappear() {
    let f = Fixture::new(&[("resource", "workspace1")]);
    f.writer.write(|c|{
        for (id,scope,due) in [("later",f.project.as_str(),5000),("earlier","resource:workspace1",2000)] {
            c.execute("INSERT INTO schedule_entries(id,scope_ref,revision,status,due_at,kind,subject_ref,origin,created_at) VALUES(?1,?2,1,'scheduled',?3,'reminder','fixture','user_explicit',1000)",rusqlite::params![id,scope,due]).map_err(crate::database_error)?;
        }Ok(())
    }).unwrap();
    let service = f.service().with_sources(Arc::new(
        crate::situation::SituationRuntime::new(Default::default(), None).unwrap(),
    ));
    let frame = service
        .prepare_frame(f.request(f.access(), vec![], None))
        .unwrap();
    let deadlines = &frame
        .frame()
        .sources
        .iter()
        .find(|s| s.kind == WorldSourceKind::Schedule)
        .unwrap()
        .entries;
    assert_eq!(
        deadlines
            .iter()
            .map(|e| e.source_id.as_str())
            .collect::<Vec<_>>(),
        vec!["earlier", "later"]
    );
    f.writer
        .write(|c| {
            c.execute(
                "UPDATE schedule_entries SET status='withdrawn' WHERE id='earlier'",
                [],
            )
            .map_err(crate::database_error)?;
            Ok(())
        })
        .unwrap();
    assert_eq!(
        service.validate_result(&frame).unwrap(),
        FrameValidity::Changed
    );
    let refreshed = service.refresh(&frame).unwrap();
    assert_eq!(
        refreshed
            .frame()
            .sources
            .iter()
            .find(|s| s.kind == WorldSourceKind::Schedule)
            .unwrap()
            .entries[0]
            .source_id,
        "later"
    );
}
