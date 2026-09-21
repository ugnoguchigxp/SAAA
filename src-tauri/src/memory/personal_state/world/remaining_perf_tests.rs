#![cfg(test)]
use super::runtime_test_support::Fixture;
use std::{sync::Arc, time::Instant};
#[test]
#[ignore = "explicit same-process performance measurement"]
fn wr_t23_source_frame_additional_p95() {
    let ids: Vec<String> = (0..8).map(|i| format!("perf{i}")).collect();
    let targets: Vec<_> = ids.iter().map(|id| ("task", id.as_str())).collect();
    let f = Fixture::with_entities(&targets, 100);
    f.writer.write(|c|{
        c.execute("INSERT INTO coding_workspaces(id,conversation_id,path) VALUES('perf-workspace',?1,'/tmp/perf')",[crate::PRIMARY_CONVERSATION_ID]).map_err(crate::database_error)?;
        for id in &ids {
            let source=format!("source-{id}");
            super::test_support::insert_source(c,&f.project,&source,"performance task source");
            c.execute("UPDATE personal_jobs SET status='completed' WHERE source_sequence IN (SELECT sequence FROM personal_sources WHERE message_id=?1)",[&source]).map_err(crate::database_error)?;
            c.execute("INSERT INTO coding_jobs(id,conversation_id,source_id,workspace_id,workspace_path,settings_json,revision,session_path,state,current_run_id) VALUES(?1,?2,?3,'perf-workspace','/tmp/perf','{}',1,?4,'settled',?5)",rusqlite::params![id,crate::PRIMARY_CONVERSATION_ID,source,format!("/tmp/{id}"),format!("run-{id}")]).map_err(crate::database_error)?;
            c.execute("INSERT INTO coding_runs(id,job_id,source_id,host_run_id,payload,digest,delivery,state,started_at) VALUES(?1,?2,?3,'host','{}','digest','accepted','settled','1000')",rusqlite::params![format!("run-{id}"),id,source]).map_err(crate::database_error)?;
        }
        crate::memory::personal_state::store::rebuild(c,crate::memory::personal_state::now())?;
        for i in 0..8 {c.execute("INSERT INTO schedule_entries(id,scope_ref,revision,status,due_at,kind,subject_ref,origin,created_at) VALUES(?1,?2,1,'scheduled',?3,'reminder','fixture','user_explicit',1000)",rusqlite::params![format!("deadline{i}"),f.project,1200+i]).map_err(crate::database_error)?;}
        Ok(())
    }).unwrap();
    if f.ledger_count() < 2000 {
        f.fill_coverage(2000 - f.ledger_count() as usize);
    }
    assert_eq!(f.ledger_count(), 2000);
    let baseline = f.service();
    let current = f.service().with_sources(Arc::new(
        crate::situation::SituationRuntime::new(Default::default(), None).unwrap(),
    ));
    let measure = |service: &super::runtime_frame::WorldFrameService| {
        let mut samples = Vec::new();
        for i in 0..35 {
            let start = Instant::now();
            let frame = service
                .prepare_frame(f.request(
                    f.access(),
                    ids.iter().map(|id| f.coding_ref(id)).collect(),
                    Some(f.graph_request("ent0")),
                ))
                .unwrap();
            assert!(serde_json::to_vec(frame.frame()).unwrap().len() <= 8192);
            assert_eq!(
                service.revalidate_frame(&frame).unwrap(),
                saaa_personal_state_core::world::runtime_frame::FrameValidity::Current
            );
            if i >= 5 {
                samples.push(start.elapsed().as_secs_f64() * 1000.0);
            }
        }
        samples
    };
    let old = measure(&baseline);
    let new = measure(&current);
    let p95 = |values: &[f64]| {
        let mut values = values.to_vec();
        values.sort_by(f64::total_cmp);
        values[28]
    };
    let delta = p95(&new) - p95(&old);
    let report = serde_json::json!({"profile":"debug","warmup":5,"samples":30,"ledger":f.ledger_count(),"projected_entities":100,"coding_sources":8,"schedule_sources":8,"baseline_ms":old,"source_frame_ms":new,"additional_p95_ms":delta,"includes":"prepare+serialize+revalidate","pass":delta<=30.0});
    println!("WORLD_REMAINING_PERF={report}");
    assert!(delta <= 30.0, "additional p95: {delta}ms");
}
