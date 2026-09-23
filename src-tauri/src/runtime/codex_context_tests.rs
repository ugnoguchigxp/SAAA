#![cfg(test)]
use super::codex_context::Dispatch;
use crate::memory::personal_state::world::runtime_test_support::{Fixture, RUN_ID};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

#[test]
fn wd_09_owner_changes_before_dispatch_and_before_acceptance_are_rejected() {
    let f = Fixture::new(&[("task", "j1")]);
    f.add_coding_job(7, "running", "running", "accepted");
    let mut dispatch = Dispatch::new(f.writer.clone(), RUN_ID.into());
    let context = dispatch.prepare().unwrap();
    let thread = json!({"params":{"developerInstructions":context}});
    let turn = json!({"params":{"input":[{"type":"text","text":"hello"}]}});
    f.set_coding_state(7, "cancel_requested", "stopping", "accepted");
    assert!(dispatch.dispatch(&thread, &turn).is_err());
    let mut dispatch = Dispatch::new(f.writer.clone(), RUN_ID.into());
    let thread = json!({"params":{"developerInstructions":dispatch.prepare().unwrap()}});
    dispatch.dispatch(&thread, &turn).unwrap();
    f.set_coding_state(8, "running", "running", "accepted");
    assert!(dispatch.finish(true).is_err());
}

#[cfg(unix)]
#[test]
fn wd_09_codex_wire_body_has_matching_digest_and_source_receipt() {
    let _lock = crate::test_environment::codex_lock()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    use std::os::unix::fs::PermissionsExt;
    let f = Fixture::new(&[("task", "j1")]);
    f.add_coding_job(7, "running", "running", "accepted");
    let directory = tempfile::tempdir().unwrap();
    let script = directory.path().join("codex-fixture.py");
    let log = directory.path().join("wire.jsonl");
    std::fs::write(&script, format!(r#"#!/usr/bin/env python3
import json, sys
for line in sys.stdin:
    m = json.loads(line)
    with open({}, 'a') as f: f.write(json.dumps(m) + '\n')
    if m.get('method') == 'initialize':
        print(json.dumps({{'id':m['id'],'result':{{}}}}), flush=True)
    if m.get('method') == 'thread/start':
        print(json.dumps({{'id':m['id'],'result':{{'thread':{{'id':'thread-fixture'}}}}}}), flush=True)
    if m.get('method') == 'turn/start':
        print(json.dumps({{'id':m['id'],'result':{{'turn':{{'id':'turn-fixture'}}}}}}), flush=True)
        print(json.dumps({{'method':'item/completed','params':{{'threadId':'thread-fixture','turnId':'turn-fixture','item':{{'id':'message-fixture','type':'agentMessage','text':'done'}}}}}}), flush=True)
        print(json.dumps({{'method':'turn/completed','params':{{'threadId':'thread-fixture','turnId':'turn-fixture','turn':{{'id':'turn-fixture','threadId':'thread-fixture','status':'completed'}}}}}}), flush=True)
"#, serde_json::to_string(&log.to_string_lossy()).unwrap())).unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    let _env = crate::test_environment::EnvGuard::set("SAAA_CODEX_PATH", &script);
    let mut dispatch = Dispatch::new(f.writer.clone(), RUN_ID.into());
    let sink = tauri::ipc::Channel::<crate::ipc_contract::RuntimeEvent>::new(|_| Ok(()));
    super::codex_process::run_codex_turn_process_with_dispatch(
        RUN_ID,
        "hello",
        directory.path(),
        "fixture",
        None,
        "",
        super::contracts::RunSupervisionPolicy::for_route(5000).unwrap(),
        &sink,
        &crate::RunCancellation::default(),
        Some(&mut dispatch),
        false,
    )
    .unwrap();
    let messages = std::fs::read_to_string(log)
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str::<Value>(l).unwrap())
        .collect::<Vec<_>>();
    let thread = messages
        .iter()
        .find(|v| v["method"] == "thread/start")
        .unwrap();
    let turn = messages
        .iter()
        .find(|v| v["method"] == "turn/start")
        .unwrap();
    assert!(!messages.iter().any(|v| v["method"] == "thread/resume"));
    assert!(thread["params"]["developerInstructions"]
        .as_str()
        .unwrap()
        .starts_with(crate::CODEX_READ_ONLY_SYSTEM_CONTEXT));
    assert!(thread["params"]["developerInstructions"]
        .as_str()
        .unwrap()
        .contains("coding_jobs:j1@7"));
    f.writer.read_serialized(|c| {
        let (request, envelope, status): (String,String,String) = c.query_row("SELECT request_digest,envelope_digest,status FROM context_generations ORDER BY ordinal DESC LIMIT 1", [], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?))).unwrap();
        assert_eq!(request, format!("{:x}", Sha256::digest(serde_json::to_vec(turn).unwrap())));
        assert_eq!(envelope, format!("{:x}", Sha256::digest(serde_json::to_vec(thread).unwrap())));
        assert_eq!(status,"completed");
        assert_eq!(c.query_row("SELECT COUNT(*) FROM context_generation_inputs WHERE source_kind='world-source-snapshot' AND selected=1", [], |r| r.get::<_,i64>(0)).unwrap(),1);
        Ok(())
    }).unwrap();
}
