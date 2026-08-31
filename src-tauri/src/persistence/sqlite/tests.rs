use super::{SqliteReaders, SqliteWriter};
use rusqlite::Connection;
use std::{
    fs,
    process::{Command, Stdio},
    sync::{Arc, Barrier},
    thread,
    time::{Duration, Instant},
};

fn initialized_writer() -> Arc<SqliteWriter> {
    let connection = Connection::open_in_memory().expect("database opens");
    crate::persistence::schema::initialize_database(&connection).expect("database initializes");
    Arc::new(SqliteWriter::from_connection(connection))
}

fn spawn_writer_helper(
    database_path: &std::path::Path,
    ready_path: &std::path::Path,
    release_path: &std::path::Path,
) -> std::process::Child {
    Command::new(std::env::current_exe().expect("test executable resolves"))
        .args([
            "--exact",
            "persistence::sqlite::tests::writer_process_helper",
            "--ignored",
            "--nocapture",
        ])
        .env("SAAA_SQLITE_WRITER_HELPER_DATABASE", database_path)
        .env("SAAA_SQLITE_WRITER_HELPER_READY", ready_path)
        .env("SAAA_SQLITE_WRITER_HELPER_RELEASE", release_path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("writer helper starts")
}

fn wait_until_ready(child: &mut std::process::Child, ready_path: &std::path::Path) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !ready_path.exists() && Instant::now() < deadline {
        if child.try_wait().expect("helper status reads").is_some() {
            break;
        }
        thread::sleep(Duration::from_millis(10));
    }
    assert!(
        ready_path.exists(),
        "writer helper did not acquire ownership"
    );
}

fn release_helper(child: std::process::Child, release_path: &std::path::Path) {
    fs::write(release_path, b"release").expect("helper release writes");
    let output = child.wait_with_output().expect("writer helper exits");
    assert!(
        output.status.success(),
        "writer helper failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn writer_reuses_one_connection_across_calls() {
    let writer = initialized_writer();
    writer
        .write(|connection| {
            connection
                .execute("CREATE TABLE writer_fixture(value INTEGER NOT NULL)", [])
                .map_err(crate::database_error)?;
            Ok(())
        })
        .expect("first write succeeds");
    writer
        .write(|connection| {
            connection
                .execute("INSERT INTO writer_fixture(value) VALUES(1)", [])
                .map_err(crate::database_error)?;
            Ok(())
        })
        .expect("second write succeeds");
    let count: i64 = writer
        .read_serialized(|connection| {
            connection
                .query_row("SELECT COUNT(*) FROM writer_fixture", [], |row| row.get(0))
                .map_err(crate::database_error)
        })
        .expect("row count loads");
    assert_eq!(count, 1);
}

#[test]
fn writer_serializes_concurrent_operations() {
    let writer = initialized_writer();
    writer
        .write(|connection| {
            connection
                .execute("CREATE TABLE writer_fixture(value INTEGER NOT NULL)", [])
                .map_err(crate::database_error)?;
            Ok(())
        })
        .expect("fixture creates");
    let threads = (0..100)
        .map(|value| {
            let writer = writer.clone();
            std::thread::spawn(move || {
                writer.write(|connection| {
                    connection
                        .execute("INSERT INTO writer_fixture(value) VALUES(?1)", [value])
                        .map_err(crate::database_error)?;
                    Ok(())
                })
            })
        })
        .collect::<Vec<_>>();
    for thread in threads {
        thread
            .join()
            .expect("thread joins")
            .expect("write succeeds");
    }
    let count: i64 = writer
        .read_serialized(|connection| {
            connection
                .query_row("SELECT COUNT(*) FROM writer_fixture", [], |row| row.get(0))
                .map_err(crate::database_error)
        })
        .expect("row count loads");
    assert_eq!(count, 100);
}

#[test]
fn writer_transaction_rolls_back_on_error() {
    let writer = initialized_writer();
    writer
        .write(|connection| {
            connection
                .execute("CREATE TABLE rollback_fixture(value INTEGER NOT NULL)", [])
                .map_err(crate::database_error)?;
            Ok(())
        })
        .expect("fixture creates");
    let error = writer
        .write_transaction(rusqlite::TransactionBehavior::Immediate, |transaction| {
            transaction
                .execute("INSERT INTO rollback_fixture(value) VALUES(1)", [])
                .map_err(crate::database_error)?;
            Err::<(), _>("rollback requested".to_string())
        })
        .expect_err("transaction fails");
    assert_eq!(error, "rollback requested");
    let count = writer
        .read_serialized(|connection| {
            connection
                .query_row("SELECT COUNT(*) FROM rollback_fixture", [], |row| {
                    row.get::<_, i64>(0)
                })
                .map_err(crate::database_error)
        })
        .expect("row count loads");
    assert_eq!(count, 0);
}

#[test]
fn reader_open_is_read_only() {
    let directory = tempfile::tempdir().expect("temporary directory creates");
    let path = directory.path().join("reader.sqlite3");
    let writer = SqliteWriter::open(&path).expect("writer opens");
    writer
        .write(|connection| {
            connection
                .execute("CREATE TABLE reader_fixture(value INTEGER NOT NULL)", [])
                .map_err(crate::database_error)?;
            Ok(())
        })
        .expect("fixture creates");
    let readers = SqliteReaders::open(&path).expect("readers open");
    assert_eq!(readers.lane_count(), 2);
    let error = readers
        .read(|connection| {
            connection
                .execute("INSERT INTO reader_fixture(value) VALUES(1)", [])
                .map(|_| ())
                .map_err(crate::database_error)
        })
        .expect_err("reader write is rejected");
    assert!(!error.is_empty());
    let temporary_error = readers
        .read(|connection| {
            connection
                .execute("CREATE TEMP TABLE forbidden(value INTEGER)", [])
                .map(|_| ())
                .map_err(crate::database_error)
        })
        .expect_err("reader temporary write is rejected");
    assert!(!temporary_error.is_empty());
}

#[test]
fn eight_readers_progress_while_the_writer_commits() {
    let directory = tempfile::tempdir().expect("temporary directory creates");
    let path = directory.path().join("concurrent.sqlite3");
    let writer = Arc::new(SqliteWriter::open(&path).expect("writer opens"));
    writer
        .write(|connection| {
            connection
                .execute(
                    "CREATE TABLE concurrent_fixture(value INTEGER NOT NULL)",
                    [],
                )
                .map_err(crate::database_error)?;
            Ok(())
        })
        .expect("fixture creates");
    let readers = SqliteReaders::open(&path).expect("readers open");
    let barrier = Arc::new(Barrier::new(10));
    let writer_thread = {
        let writer = writer.clone();
        let barrier = barrier.clone();
        thread::spawn(move || {
            barrier.wait();
            for value in 0..100 {
                writer
                    .write(|connection| {
                        connection
                            .execute("INSERT INTO concurrent_fixture(value) VALUES(?1)", [value])
                            .map_err(crate::database_error)?;
                        Ok(())
                    })
                    .expect("concurrent write succeeds");
            }
        })
    };
    let reader_threads = (0..8)
        .map(|_| {
            let readers = readers.clone();
            let barrier = barrier.clone();
            thread::spawn(move || {
                barrier.wait();
                for _ in 0..50 {
                    readers
                        .read(|connection| {
                            connection
                                .query_row("SELECT COUNT(*) FROM concurrent_fixture", [], |row| {
                                    row.get::<_, i64>(0)
                                })
                                .map_err(crate::database_error)
                        })
                        .expect("concurrent read succeeds");
                }
            })
        })
        .collect::<Vec<_>>();
    barrier.wait();
    writer_thread.join().expect("writer thread joins");
    for reader in reader_threads {
        reader.join().expect("reader thread joins");
    }
    let count = readers
        .read(|connection| {
            connection
                .query_row("SELECT COUNT(*) FROM concurrent_fixture", [], |row| {
                    row.get::<_, i64>(0)
                })
                .map_err(crate::database_error)
        })
        .expect("final count loads");
    assert_eq!(count, 100);
}

#[test]
fn one_reader_operation_observes_one_consistent_snapshot() {
    let directory = tempfile::tempdir().expect("temporary directory creates");
    let path = directory.path().join("snapshot.sqlite3");
    let writer = Arc::new(SqliteWriter::open(&path).expect("writer opens"));
    writer
        .write(|connection| {
            connection
                .execute("CREATE TABLE snapshot_fixture(value INTEGER NOT NULL)", [])
                .map_err(crate::database_error)?;
            connection
                .execute("INSERT INTO snapshot_fixture(value) VALUES(0)", [])
                .map_err(crate::database_error)?;
            Ok(())
        })
        .expect("fixture creates");
    let readers = SqliteReaders::open(&path).expect("readers open");
    let (first_read_sender, first_read_receiver) = std::sync::mpsc::sync_channel(1);
    let (writer_done_sender, writer_done_receiver) = std::sync::mpsc::sync_channel(1);
    let reader = thread::spawn(move || {
        readers.read(|connection| {
            let first = connection
                .query_row("SELECT value FROM snapshot_fixture", [], |row| {
                    row.get::<_, i64>(0)
                })
                .map_err(crate::database_error)?;
            first_read_sender
                .send(())
                .map_err(|_| "Could not signal first read".to_string())?;
            writer_done_receiver
                .recv_timeout(Duration::from_secs(5))
                .map_err(|_| "Writer did not finish".to_string())?;
            let second = connection
                .query_row("SELECT value FROM snapshot_fixture", [], |row| {
                    row.get::<_, i64>(0)
                })
                .map_err(crate::database_error)?;
            Ok((first, second))
        })
    });
    first_read_receiver
        .recv_timeout(Duration::from_secs(5))
        .expect("reader starts its snapshot");
    writer
        .write(|connection| {
            connection
                .execute("UPDATE snapshot_fixture SET value=1", [])
                .map_err(crate::database_error)?;
            Ok(())
        })
        .expect("writer commits during read snapshot");
    writer_done_sender
        .send(())
        .expect("writer completion signals");

    assert_eq!(
        reader
            .join()
            .expect("reader thread joins")
            .expect("reader succeeds"),
        (0, 0)
    );
    let latest = SqliteReaders::open(&path)
        .expect("readers open")
        .read(|connection| {
            connection
                .query_row("SELECT value FROM snapshot_fixture", [], |row| {
                    row.get::<_, i64>(0)
                })
                .map_err(crate::database_error)
        })
        .expect("later reader sees latest commit");
    assert_eq!(latest, 1);
}

#[test]
fn settings_snapshot_cache_ignores_unrelated_writes_and_refreshes_on_settings_change() {
    let directory = tempfile::tempdir().expect("temporary directory creates");
    let path = directory.path().join("settings-cache.sqlite3");
    let writer = SqliteWriter::open(&path).expect("writer opens");
    let readers = SqliteReaders::open(&path).expect("readers open");
    let first = readers
        .read(|connection| readers.settings_snapshot(connection))
        .expect("initial settings snapshot loads");
    let first_revision = readers
        .cached_settings_revision()
        .expect("settings snapshot is cached");

    writer
        .write(|connection| {
            connection
                .execute(
                    "UPDATE conversations SET updated_at=?1 WHERE id=?2",
                    [crate::now_iso(), crate::PRIMARY_CONVERSATION_ID.to_string()],
                )
                .map_err(crate::database_error)?;
            Ok(())
        })
        .expect("unrelated write succeeds");
    let unchanged = readers
        .read(|connection| readers.settings_snapshot(connection))
        .expect("cached settings snapshot loads");
    assert_eq!(readers.cached_settings_revision(), Some(first_revision));
    assert_eq!(unchanged.len(), first.len());

    writer
        .write(|connection| {
            connection
                .execute(
                    "UPDATE settings_documents
                     SET value_json=json_set(value_json, '$.listeningEnabled', json('true')), updated_at=?1
                     WHERE namespace='voice.runtime' AND key='default'",
                    [crate::now_iso()],
                )
                .map_err(crate::database_error)?;
            Ok(())
        })
        .expect("settings write succeeds");
    let refreshed = readers
        .read(|connection| readers.settings_snapshot(connection))
        .expect("refreshed settings snapshot loads");
    assert!(readers.cached_settings_revision().unwrap() > first_revision);
    let voice = refreshed
        .iter()
        .find(|document| document.namespace == "voice.runtime")
        .expect("voice settings exist");
    assert_eq!(voice.value_json["listeningEnabled"], true);
}

#[test]
fn second_process_is_rejected_before_database_open() {
    let directory = tempfile::tempdir().expect("temporary directory creates");
    let database_path = directory.path().join("saaa.sqlite3");
    let ready_path = directory.path().join("writer.ready");
    let release_path = directory.path().join("writer.release");
    let mut child = spawn_writer_helper(&database_path, &ready_path, &release_path);
    wait_until_ready(&mut child, &ready_path);

    let error = match SqliteWriter::open(&database_path) {
        Ok(_) => panic!("second writer was accepted"),
        Err(error) => error,
    };
    assert!(matches!(
        error,
        super::writer::DatabaseOpenError::AlreadyOwned
    ));

    release_helper(child, &release_path);
}

#[test]
fn second_writer_in_the_same_process_is_rejected() {
    let directory = tempfile::tempdir().expect("temporary directory creates");
    let database_path = directory.path().join("saaa.sqlite3");
    let _writer = SqliteWriter::open(&database_path).expect("first writer opens");

    let error = match SqliteWriter::open(&database_path) {
        Ok(_) => panic!("second writer in the same process was accepted"),
        Err(error) => error,
    };
    assert!(matches!(
        error,
        super::writer::DatabaseOpenError::AlreadyOwned
    ));
}

#[test]
fn crashed_owner_releases_lock_without_removing_the_lock_file() {
    let directory = tempfile::tempdir().expect("temporary directory creates");
    let database_path = directory.path().join("saaa.sqlite3");
    let ready_path = directory.path().join("writer.ready");
    let release_path = directory.path().join("writer.release");
    let mut child = spawn_writer_helper(&database_path, &ready_path, &release_path);
    wait_until_ready(&mut child, &ready_path);
    child.kill().expect("writer helper terminates");
    child.wait().expect("writer helper is reaped");

    let lock_path = directory.path().join("saaa.sqlite3.writer.lock");
    assert!(lock_path.exists(), "lock file remains after owner crash");
    let writer = SqliteWriter::open(&database_path).expect("successor acquires ownership");
    drop(writer);
    SqliteWriter::open(&database_path).expect("later restart acquires ownership");
}

#[test]
fn only_the_owner_process_runs_migration_backup_and_bootstrap() {
    let directory = tempfile::tempdir().expect("temporary directory creates");
    let database_path = directory.path().join("saaa.sqlite3");
    let connection = Connection::open(&database_path).expect("fixture database opens");
    crate::persistence::schema::initialize_database(&connection).expect("fixture initializes");
    connection
        .pragma_update(None, "user_version", 14)
        .expect("fixture version downgrades");
    drop(connection);

    let ready_path = directory.path().join("writer.ready");
    let release_path = directory.path().join("writer.release");
    let mut child = spawn_writer_helper(&database_path, &ready_path, &release_path);
    wait_until_ready(&mut child, &ready_path);
    assert!(matches!(
        SqliteWriter::open(&database_path),
        Err(super::writer::DatabaseOpenError::AlreadyOwned)
    ));
    release_helper(child, &release_path);

    let backups = fs::read_dir(directory.path().join("backups"))
        .expect("migration backup directory reads")
        .collect::<Result<Vec<_>, _>>()
        .expect("migration backups read");
    assert_eq!(backups.len(), 1);
    let reopened = Connection::open(&database_path).expect("migrated database reopens");
    let version: i64 = reopened
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .expect("schema version reads");
    assert_eq!(version, crate::memory::control_plane::MEMORY_SCHEMA_VERSION);
    let ready_events: i64 = reopened
        .query_row(
            "SELECT COUNT(*) FROM audit_events WHERE event_name='database-ready'",
            [],
            |row| row.get(0),
        )
        .expect("database-ready events count");
    assert_eq!(
        ready_events, 2,
        "fixture bootstrap plus owner bootstrap only"
    );
}

#[cfg(unix)]
#[test]
fn lock_file_is_private_regular_and_owned_by_the_current_user() {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};

    let directory = tempfile::tempdir().expect("temporary directory creates");
    let database_path = directory.path().join("saaa.sqlite3");
    let _writer = SqliteWriter::open(&database_path).expect("writer opens");
    let metadata = fs::symlink_metadata(directory.path().join("saaa.sqlite3.writer.lock"))
        .expect("lock metadata reads");
    assert!(metadata.file_type().is_file());
    assert!(!metadata.file_type().is_symlink());
    assert_eq!(metadata.nlink(), 1);
    assert_eq!(metadata.permissions().mode() & 0o777, 0o600);
    assert_eq!(metadata.uid(), unsafe { libc::geteuid() });
}

#[cfg(unix)]
#[test]
fn symlink_lock_file_is_rejected() {
    use std::os::unix::fs::symlink;

    let directory = tempfile::tempdir().expect("temporary directory creates");
    let database_path = directory.path().join("saaa.sqlite3");
    let target_path = directory.path().join("lock-target");
    fs::write(&target_path, b"target").expect("lock target writes");
    symlink(
        &target_path,
        directory.path().join("saaa.sqlite3.writer.lock"),
    )
    .expect("lock symlink creates");

    assert!(matches!(
        SqliteWriter::open(&database_path),
        Err(super::writer::DatabaseOpenError::OwnershipUnavailable)
    ));
}

#[cfg(unix)]
#[test]
fn hard_linked_lock_file_is_rejected() {
    use std::os::unix::fs::PermissionsExt;

    let directory = tempfile::tempdir().expect("temporary directory creates");
    let database_path = directory.path().join("saaa.sqlite3");
    let target_path = directory.path().join("lock-target");
    fs::write(&target_path, b"target").expect("lock target writes");
    fs::set_permissions(&target_path, fs::Permissions::from_mode(0o600))
        .expect("lock permissions set");
    fs::hard_link(
        &target_path,
        directory.path().join("saaa.sqlite3.writer.lock"),
    )
    .expect("lock hard link creates");

    assert!(matches!(
        SqliteWriter::open(&database_path),
        Err(super::writer::DatabaseOpenError::OwnershipUnavailable)
    ));
}

#[cfg(unix)]
#[test]
fn non_private_lock_file_is_rejected() {
    use std::os::unix::fs::PermissionsExt;

    let directory = tempfile::tempdir().expect("temporary directory creates");
    let database_path = directory.path().join("saaa.sqlite3");
    let lock_path = directory.path().join("saaa.sqlite3.writer.lock");
    fs::write(&lock_path, b"lock").expect("lock file writes");
    fs::set_permissions(&lock_path, fs::Permissions::from_mode(0o644))
        .expect("lock permissions set");

    assert!(matches!(
        SqliteWriter::open(&database_path),
        Err(super::writer::DatabaseOpenError::OwnershipUnavailable)
    ));
}

#[test]
#[ignore = "subprocess helper"]
fn writer_process_helper() {
    let Some(database_path) = std::env::var_os("SAAA_SQLITE_WRITER_HELPER_DATABASE") else {
        return;
    };
    let ready_path = std::env::var_os("SAAA_SQLITE_WRITER_HELPER_READY")
        .expect("helper ready path is configured");
    let release_path = std::env::var_os("SAAA_SQLITE_WRITER_HELPER_RELEASE")
        .expect("helper release path is configured");
    let _writer = SqliteWriter::open(std::path::Path::new(&database_path))
        .expect("helper writer acquires ownership");
    fs::write(&ready_path, b"ready").expect("helper ready writes");
    let deadline = Instant::now() + Duration::from_secs(10);
    while !std::path::Path::new(&release_path).exists() && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(10));
    }
    assert!(
        std::path::Path::new(&release_path).exists(),
        "helper release timed out"
    );
}
