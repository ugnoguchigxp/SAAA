#[allow(dead_code)]
#[path = "../src/providers/llm_websocket/protocol.rs"]
mod protocol;

use std::{fs, hint::black_box, path::PathBuf, time::Instant};

use protocol::OrderedRun;
use serde_json::json;

const REPETITIONS: usize = 1_000;
const DELTAS_PER_RUN: u64 = 10_000;
const LIMIT_NS: u64 = 250_000_000;

fn percentile(sorted: &[u64], percent: usize) -> u64 {
    sorted[((sorted.len() * percent).div_ceil(100)).saturating_sub(1)]
}

fn bootstrap_p95_ci(samples: &[u64]) -> [u64; 2] {
    let mut seed = 0x5aaa_2026_u64;
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

fn max_rss_bytes() -> u64 {
    let mut usage = std::mem::MaybeUninit::<libc::rusage>::zeroed();
    if unsafe { libc::getrusage(libc::RUSAGE_SELF, usage.as_mut_ptr()) } != 0 {
        return 0;
    }
    let rss = unsafe { usage.assume_init() }.ru_maxrss.max(0) as u64;
    if cfg!(target_os = "macos") {
        rss
    } else {
        rss.saturating_mul(1_024)
    }
}

fn frame(seq: u64, payload: &[u8]) -> Vec<u8> {
    let mut frame = Vec::with_capacity(16 + payload.len());
    frame.extend_from_slice(b"SAD1");
    frame.extend_from_slice(&[1, 0]);
    frame.extend_from_slice(&16_u16.to_be_bytes());
    frame.extend_from_slice(&seq.to_be_bytes());
    frame.extend_from_slice(payload);
    frame
}

fn run_once(payload: &[u8]) -> u64 {
    let started = Instant::now();
    let mut run = OrderedRun::new("run_benchmark").expect("valid benchmark run");
    run.accept_text(
        r#"{"type":"run.accepted","runId":"run_benchmark","seq":1,"providerRunId":"provider_benchmark","model":"benchmark"}"#,
    )
    .expect("accepted benchmark run");
    for seq in 2..=DELTAS_PER_RUN + 1 {
        black_box(run.accept_binary(black_box(&frame(seq, payload))))
            .expect("ordered binary delta");
    }
    assert_eq!(run.content().len(), payload.len() * DELTAS_PER_RUN as usize);
    started.elapsed().as_nanos().try_into().unwrap_or(u64::MAX)
}

fn main() {
    let payload = "😀😀😀😀日a".as_bytes();
    assert_eq!(payload.len(), 20);
    for _ in 0..5 {
        black_box(run_once(payload));
    }
    let rss_before = max_rss_bytes();
    let mut histogram = (0..REPETITIONS)
        .map(|_| run_once(payload))
        .collect::<Vec<_>>();
    let rss_growth = max_rss_bytes().saturating_sub(rss_before);
    histogram.sort_unstable();
    let p95 = percentile(&histogram, 95);
    let p95_ci = bootstrap_p95_ci(&histogram);
    let passed = p95 <= LIMIT_NS && rss_growth <= 16 * 1_024 * 1_024;
    let report = json!({
        "format": "saaa-performance-gate-v1", "profile": "release", "gate": "G-LLM-02",
        "hardware": { "arch": std::env::consts::ARCH, "os": std::env::consts::OS },
        "sampleCount": histogram.len(), "deltaBytes": payload.len(), "deltasPerRun": DELTAS_PER_RUN,
        "p50Ns": percentile(&histogram, 50), "p95Ns": p95, "p99Ns": percentile(&histogram, 99),
        "p95Bootstrap95CiNs": p95_ci,
        "peakRssGrowthBytes": rss_growth, "limitNs": LIMIT_NS, "rssLimitBytes": 16 * 1_024 * 1_024,
        "rawHistogramNs": histogram, "passed": passed
    });
    let path = std::env::var_os("SAAA_PERFORMANCE_REPORT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("target/performance"));
    fs::create_dir_all(&path).expect("performance report directory");
    fs::write(
        path.join("streaming-hot-path.json"),
        format!("{report:#}\n"),
    )
    .expect("streaming performance report");
    println!(
        "{}",
        json!({ "gate": "G-LLM-02", "p95Ns": p95, "passed": passed })
    );
    if !passed {
        std::process::exit(1);
    }
}
