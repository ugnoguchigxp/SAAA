use crate::persistence::schema::initialize_database;
use crate::steward::pump::{self, Wake};
use crate::test_support::app_state;
use crate::PRIMARY_CONVERSATION_ID;
use rusqlite::Connection;
use std::time::Instant;

fn p95(samples: &mut [u128]) -> u128 {
    samples.sort_unstable();
    let index = ((samples.len() as f64) * 0.95).ceil() as usize;
    samples[index.saturating_sub(1).min(samples.len() - 1)]
}

#[test]
fn dw_r25_repeated_wakes_have_no_duplicate_effect() {
    let wake = Wake::default();
    wake.signal();
    wake.signal();
    assert!(wake.take_pending());
    assert!(!wake.take_pending());
}

#[test]
fn dw_r25_driver_and_drain_p95_under_two_seconds() {
    let mut driver = Vec::with_capacity(32);
    let mut drain = Vec::with_capacity(32);
    for _ in 0..32 {
        let connection = Connection::open_in_memory().expect("db");
        initialize_database(&connection).expect("init");
        let state = app_state(connection);
        let started = Instant::now();
        state
            .sqlite_writer
            .write(|connection| crate::steward::driver::consume(connection))
            .expect("driver");
        driver.push(started.elapsed().as_millis());
        let started = Instant::now();
        pump::drain(&state).expect("drain");
        drain.push(started.elapsed().as_millis());
        let _ = PRIMARY_CONVERSATION_ID;
    }
    let driver_p95 = p95(&mut driver);
    let drain_p95 = p95(&mut drain);
    assert!(
        driver_p95 <= 2_000,
        "driver p95 {driver_p95}ms exceeds 2s (n=32, max={})",
        driver.iter().max().unwrap()
    );
    assert!(
        drain_p95 <= 2_000,
        "drain p95 {drain_p95}ms exceeds 2s (n=32, max={})",
        drain.iter().max().unwrap()
    );
}
