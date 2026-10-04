use super::*;
use std::{sync::mpsc, time::Duration};

#[test]
fn a_read_uses_the_free_lane_when_the_round_robin_lane_is_busy() {
    let directory = tempfile::tempdir().expect("temporary directory creates");
    let path = directory.path().join("free-lane.sqlite3");
    let _writer = SqliteWriter::open(&path).expect("writer opens");
    let readers = SqliteReaders::open(&path).expect("readers open");
    let persistent = match &readers.source {
        ReaderSource::Persistent(readers) => readers.clone(),
        ReaderSource::Serialized(_) => unreachable!("fixture uses persistent readers"),
    };
    persistent.next_lane.store(0, Ordering::Relaxed);
    let blocked_lane = persistent.lanes[0].lock().expect("lane lock");
    let (completed, received) = mpsc::sync_channel(1);
    std::thread::spawn(move || {
        let result = readers.read(|connection| {
            connection
                .query_row("SELECT 1", [], |row| row.get::<_, i64>(0))
                .map_err(crate::database_error)
        });
        completed.send(result).expect("result sends");
    });

    assert_eq!(
        received
            .recv_timeout(Duration::from_secs(1))
            .expect("free reader lane completes")
            .expect("read succeeds"),
        1
    );
    drop(blocked_lane);
}
