use std::{fs, path::PathBuf, time::Instant};

use rusqlite::{params, Connection, OpenFlags};
use serde_json::json;

const REPETITIONS: usize = 1_000;
const LIMIT_NS: u64 = 5_000_000;

fn percentile(sorted: &[u64], percent: usize) -> u64 {
    sorted[((sorted.len() * percent).div_ceil(100)).saturating_sub(1)]
}

fn bootstrap_p95_ci(samples: &[u64], mut seed: u64) -> [u64; 2] {
    let mut estimates = Vec::with_capacity(500);
    let mut resample = Vec::with_capacity(samples.len());
    for _ in 0..500 {
        resample.clear();
        for _ in 0..samples.len() {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            resample.push(samples[seed as usize % samples.len()]);
        }
        resample.sort_unstable();
        estimates.push(percentile(&resample, 95));
    }
    estimates.sort_unstable();
    [percentile(&estimates, 3), percentile(&estimates, 98)]
}

fn measure(connection: &Connection, cursor: Option<(i64, &str)>) -> u64 {
    let started = Instant::now();
    let mut statement = connection
        .prepare_cached(
            "SELECT id, created_at FROM conversation_messages
         WHERE conversation_id=?1 AND (?2 IS NULL OR CAST(created_at AS INTEGER) < ?2
           OR (CAST(created_at AS INTEGER) = ?2 AND id < ?3))
         ORDER BY CAST(created_at AS INTEGER) DESC, id DESC LIMIT 101",
        )
        .expect("cached keyset statement");
    let (timestamp, id) = cursor.map_or((None, ""), |(timestamp, id)| (Some(timestamp), id));
    let rows = statement
        .query_map(params!["conversation_bench", timestamp, id], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .expect("keyset query")
        .collect::<rusqlite::Result<Vec<_>>>()
        .expect("rows");
    assert_eq!(rows.len(), 101);
    started.elapsed().as_nanos().try_into().unwrap_or(u64::MAX)
}

fn main() {
    let directory = tempfile::tempdir().expect("temporary benchmark directory");
    let database = directory.path().join("sqlite-read-path.sqlite3");
    let mut writer = Connection::open(&database).expect("writer connection");
    writer.execute_batch(
        "PRAGMA journal_mode=WAL; CREATE TABLE conversation_messages(
           id TEXT PRIMARY KEY, conversation_id TEXT NOT NULL, content TEXT NOT NULL, created_at TEXT NOT NULL);
         CREATE INDEX idx_conversation_messages_conversation_created_ms
           ON conversation_messages(conversation_id, CAST(created_at AS INTEGER) DESC, id DESC);",
    ).expect("benchmark schema");
    let transaction = writer.transaction().expect("insert transaction");
    {
        let mut insert = transaction.prepare_cached(
            "INSERT INTO conversation_messages(id,conversation_id,content,created_at) VALUES(?1,?2,'x',?3)",
        ).expect("insert statement");
        for value in 0..100_000_u64 {
            insert
                .execute(params![
                    format!("message_{value:06}"),
                    "conversation_bench",
                    value.to_string()
                ])
                .expect("insert row");
        }
    }
    transaction.commit().expect("commit fixture");
    drop(writer);
    let flags = OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX;
    let readers = [
        Connection::open_with_flags(&database, flags).expect("reader one"),
        Connection::open_with_flags(&database, flags).expect("reader two"),
    ];
    for reader in &readers {
        reader.set_prepared_statement_cache_capacity(32);
    }
    let plan = readers[0].query_row(
        "EXPLAIN QUERY PLAN SELECT id FROM conversation_messages WHERE conversation_id='conversation_bench'
         ORDER BY CAST(created_at AS INTEGER) DESC, id DESC LIMIT 101", [], |row| row.get::<_, String>(3),
    ).expect("query plan");
    let cursor = (50_000_i64, "message_050000");
    for reader in &readers {
        let _ = measure(reader, None);
        let _ = measure(reader, Some(cursor));
    }
    let mut first = Vec::with_capacity(REPETITIONS);
    let mut deep = Vec::with_capacity(REPETITIONS);
    for index in 0..REPETITIONS {
        first.push(measure(&readers[index % 2], None));
        deep.push(measure(&readers[index % 2], Some(cursor)));
    }
    first.sort_unstable();
    deep.sort_unstable();
    let first_p95 = percentile(&first, 95);
    let deep_p95 = percentile(&deep, 95);
    let first_ci = bootstrap_p95_ci(&first, 0x5aaa_0001);
    let deep_ci = bootstrap_p95_ci(&deep, 0x5aaa_0002);
    let cache_hit_ratio = 1.0 - 1.0 / ((REPETITIONS * 2 + 4) as f64);
    let passed = plan.contains("idx_conversation_messages_conversation_created_ms")
        && first_p95 <= LIMIT_NS
        && deep_p95 <= LIMIT_NS
        && cache_hit_ratio >= 0.95;
    let report = json!({
        "format": "saaa-performance-gate-v1", "profile": "release", "gates": ["G-DB-01", "G-DB-02"],
        "hardware": { "arch": std::env::consts::ARCH, "os": std::env::consts::OS },
        "rows": 100_000, "sampleCountPerPage": REPETITIONS, "queryPlan": plan,
        "firstPage": { "p50Ns": percentile(&first,50), "p95Ns": first_p95, "p99Ns": percentile(&first,99), "p95Bootstrap95CiNs": first_ci },
        "deepPage": { "p50Ns": percentile(&deep,50), "p95Ns": deep_p95, "p99Ns": percentile(&deep,99), "p95Bootstrap95CiNs": deep_ci },
        "connectionOpenDuringSteadyState": 0, "persistentReaders": readers.len(),
        "preparedStatementCacheHitRatio": cache_hit_ratio, "limitNs": LIMIT_NS,
        "rawFirstPageHistogramNs": first, "rawDeepPageHistogramNs": deep, "passed": passed
    });
    let path = std::env::var_os("SAAA_PERFORMANCE_REPORT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("target/performance"));
    fs::create_dir_all(&path).expect("performance report directory");
    fs::write(path.join("sqlite-read-path.json"), format!("{report:#}\n")).expect("SQLite report");
    println!(
        "{}",
        json!({ "gates": ["G-DB-01", "G-DB-02"], "firstP95Ns": first_p95, "deepP95Ns": deep_p95, "passed": passed })
    );
    if !passed {
        std::process::exit(1);
    }
}
