use super::*;

#[test]
fn synthetic_sources_preserve_speaker_time_and_host_scope() {
    let at = now() - 60000;
    let c = database(&json!({"sources":[
        {"id":"user-source","version":1,"text":"私は紅茶が好きです。","recordedAt":at},
        {"id":"proposal","version":1,"text":"提案です。","role":"assistant","scope":"project:demo","recordedAt":at}
    ]})).unwrap();
    let (role, recorded): (String, i64) = c.query_row(
        "SELECT role, recorded_at FROM personal_sources WHERE message_id='proposal' AND available=1",
        [], |r| Ok((r.get(0)?, r.get(1)?)),
    ).unwrap();
    assert_eq!((role.as_str(), recorded), ("assistant", at));
    let scoped: bool = c.query_row(
        "SELECT EXISTS(SELECT 1 FROM personal_jobs j JOIN personal_sources s ON s.sequence=j.source_sequence JOIN personal_source_scope_refs r ON r.source_id=s.message_id AND r.version=s.version WHERE s.message_id='proposal' AND r.scope_key=j.scope_key AND j.scope_key='project:demo')",
        [], |r| r.get(0),
    ).unwrap();
    assert!(scoped);
    let enabled: bool = c
        .query_row(
            "SELECT enabled FROM personal_consolidation_settings WHERE id=1",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(!enabled);
}

#[test]
fn synthetic_sources_reject_unknown_speaker_and_unowned_scope() {
    for extra in [json!({"role":"system"}), json!({"scope":"user:another"})] {
        let mut source = json!({"id":"source","version":1,"text":"記録"});
        source
            .as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        assert!(database(&json!({"sources":[source]})).is_err());
    }
}
