use super::memory_contract::grant;
use super::*;

#[test]
fn legacy_iso_clock_migration_retires_bad_version_and_is_idempotent() {
    let c = db();
    let stamp = "2026-10-07T00:00:00.123Z";
    c.execute(
        "INSERT INTO conversation_messages VALUES('origin',?1,'user','私は紅茶が好きです。',?2)",
        params![crate::PRIMARY_CONVERSATION_ID, stamp],
    )
    .unwrap();
    let key = grant(&c);
    c.execute(
        "UPDATE personal_sources SET recorded_at=2026 WHERE message_id='origin'",
        [],
    )
    .unwrap();
    super::super::schema::migrate(&c).unwrap();
    let source:(u64,i64)=c.query_row("SELECT version,recorded_at FROM memory_episode_source_v1 WHERE source_id='origin' AND scope_key=?1",[key],|r|Ok((r.get(0)?,r.get(1)?))).unwrap();
    assert_eq!(source, (2, 1791331200123));
    assert_eq!(
        c.query_row(
            "SELECT available FROM personal_sources WHERE message_id='origin' AND version=1",
            [],
            |r| r.get::<_, u32>(0)
        )
        .unwrap(),
        0
    );
    super::super::schema::migrate(&c).unwrap();
    assert_eq!(
        c.query_row(
            "SELECT count(*) FROM personal_sources WHERE message_id='origin'",
            [],
            |r| r.get::<_, u32>(0)
        )
        .unwrap(),
        2
    );
    assert_eq!(c.query_row("SELECT count(*) FROM personal_jobs j JOIN personal_sources s ON s.sequence=j.source_sequence WHERE s.message_id='origin' AND s.version=2 AND j.status='queued'",[],|r|r.get::<_,u32>(0)).unwrap(),1);
}

#[test]
fn production_iso_timestamp_and_numeric_fixture_have_the_same_memory_clock() {
    let c = db();
    let stamp = "2026-10-07T00:00:00.123Z";
    let expected = 1791331200123i64;
    c.execute(
        "INSERT INTO conversation_messages VALUES('origin',?1,'user','私は紅茶が好きです。',?2)",
        params![crate::PRIMARY_CONVERSATION_ID, stamp],
    )
    .unwrap();
    let key = grant(&c);
    let times: (i64,i64) = c.query_row("SELECT recorded_at,uttered_at FROM memory_episode_source_v1 WHERE source_id='origin' AND scope_key=?1",[key],|r|Ok((r.get(0)?,r.get(1)?))).unwrap();
    assert_eq!(times, (expected, expected));
    let foreground: i64 = c
        .query_row("SELECT last_foreground_at FROM personal_scope", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(foreground, expected);
    insert(&c, "numeric", "数値形式の記録");
    let numeric: i64 = c
        .query_row(
            "SELECT recorded_at FROM personal_sources WHERE message_id='numeric'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(numeric > expected - 86400000);
}
