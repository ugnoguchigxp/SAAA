#[test]
pub(super) fn dw_r01_reopen_two_and_eight_active_goals() {
    let directory = tempfile::tempdir().expect("tmp");
    let path = directory.path().join("steward.sqlite");
    {
        let connection = Connection::open(&path).expect("open");
        initialize_database(&connection).expect("init");
        connection
            .execute(
                "INSERT INTO conversations(id,title,task_mode,created_at,updated_at) VALUES('c1','t','conversation','1','1')",
                [],
            )
            .ok();
        for index in 0..8 {
            let goal = format!("goal-{index}");
            connection
                .execute(
                    "INSERT INTO steward_goals(id,conversation_id,origin,success_condition,status,created_at)
                     VALUES(?1,'c1','user_explicit','tests','active','1')",
                    rusqlite::params![goal],
                )
                .expect("goal");
            connection
                .execute(
                    "INSERT INTO steward_delegations(id,goal_id,conversation_id,workspace_id,ops,budget_runs,budget_ms,notify,status,created_at)
                     VALUES(?1,?2,'c1','ws','read',1,1000,'both','active','1')",
                    rusqlite::params![format!("d-{index}"), format!("goal-{index}")],
                )
                .expect("delegation");
        }
        connection
            .execute(
                "INSERT INTO steward_reports(id,conversation_id,digest,flushed,created_at) VALUES('r1','c1','done',0,'1')",
                [],
            )
            .expect("report");
    }
    let connection = Connection::open(&path).expect("reopen");
    initialize_database(&connection).expect("migrate again");
    initialize_database(&connection).expect("idempotent");
    let goals: i64 = connection
        .query_row("SELECT COUNT(*) FROM steward_goals WHERE status='active'", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(goals, 8);
    let reports: i64 = connection
        .query_row("SELECT COUNT(*) FROM steward_reports", [], |row| row.get(0))
        .unwrap();
    assert_eq!(reports, 1);
    let _ = fs::metadata(&path);
}

#[test]
pub(super) fn dw_r01_upgrade_legacy_goal_and_job() {
    let directory = tempfile::tempdir().expect("tmp");
    let path = directory.path().join("legacy.sqlite");
    {
        let connection = Connection::open(&path).expect("open");
        connection.execute_batch(
            "CREATE TABLE steward_goals (
               id TEXT PRIMARY KEY, conversation_id TEXT NOT NULL, origin TEXT NOT NULL,
               success_condition TEXT NOT NULL, status TEXT NOT NULL, created_at TEXT NOT NULL, superseded_by TEXT
             );
             CREATE UNIQUE INDEX steward_one_active_goal ON steward_goals(conversation_id)
               WHERE status='active' AND superseded_by IS NULL;
             INSERT INTO steward_goals VALUES('g1','c1','user_explicit','ok','active','1',NULL);",
        )
        .expect("legacy");
    }
    let connection = Connection::open(&path).expect("reopen");
    initialize_database(&connection).expect("upgrade");
    let count: i64 = connection
        .query_row("SELECT COUNT(*) FROM steward_goals", [], |row| row.get(0))
        .unwrap();
    assert_eq!(count, 1);
    let index: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE name='steward_one_active_goal'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(index, 0);
}
