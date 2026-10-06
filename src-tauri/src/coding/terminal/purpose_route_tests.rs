#![cfg(test)]
use super::*;
#[test]
fn model_route_revocation_or_deadline_blocks_automatic_terminal_answer_adoption() {
    for withdraw in [true, false] {
        let c = fixture();
        pause(&c, "blocker");
        c.execute("INSERT INTO terminal_decisions(question_id,job_id,status) VALUES('run:q','job','reserved')",[]).unwrap();
        c.execute("INSERT INTO terminal_questions(id,run_id,job_id,kind,input_json,state,created_at) VALUES('run:second','run','job','blocker','{}','awaiting_user','1')",[]).unwrap();
        let state = crate::test_support::app_state(c);
        let context = automatic_context(&state, "run:q").unwrap();
        let route = state
            .sqlite_readers
            .read(|db| {
                let loaded = crate::persistence::service_registry_store::load_registry(db)?;
                crate::providers::service_registry::resolve_route(
                    &loaded.snapshot,
                    crate::providers::service_registry::Purpose::ConversationRespond,
                    Default::default(),
                )
                .map_err(|e| format!("{e:?}"))
            })
            .unwrap();
        if withdraw {
            state
                .sqlite_writer
                .write(|db| {
                    let loaded = crate::persistence::service_registry_store::load_registry(db)?;
                    let mut next = loaded.snapshot;
                    next.bindings
                        .iter_mut()
                        .find(|b| {
                            b.purpose
                                == crate::providers::service_registry::Purpose::ConversationRespond
                        })
                        .unwrap()
                        .enabled = false;
                    crate::persistence::service_registry_store::save_registry(
                        db,
                        &next,
                        loaded.revision,
                    )?;
                    Ok(())
                })
                .unwrap();
        }
        let deadline = tokio::time::Instant::now()
            + std::time::Duration::from_millis(if withdraw { 1000 } else { 0 });
        let output =
            json!({"action":"answer","sourceId":"human","quote":"Use Blue","answer":"Blue"});
        assert!(automatic_answer(&state, &context, &output, Some((&route, deadline))).is_err());
        state.sqlite_readers.read(|db| {
            let status:String=db.query_row("SELECT state FROM terminal_questions WHERE id='run:q'",[],|r|r.get(0)).unwrap();assert_eq!(status,"awaiting_user");
            let count:i64=db.query_row("SELECT count(*) FROM audit_events WHERE event_name='purpose-route-accepted'",[],|r|r.get(0)).unwrap();assert_eq!(count,0);Ok(())
        }).unwrap();
    }
}
