//! Background observation of the LAN Provider Harness. Selection reads only the snapshot.
use super::reachability::{PROBE_INTERVAL_SECS, PROBE_TIMEOUT_MS};
use crate::persistence::load_model_providers;
use crate::AppState;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::Arc;
use std::time::{Duration, Instant};

pub(crate) fn spawn(state: &AppState) -> tauri::async_runtime::JoinHandle<()> {
    let reachability = Arc::clone(&state.reachability);
    let kick = Arc::clone(&state.reachability_kick);
    let readers = state.sqlite_readers.clone();
    tauri::async_runtime::spawn(async move {
        let mut interfaces = interface_fingerprint();
        let mut probe_interval = tokio::time::interval(Duration::from_secs(PROBE_INTERVAL_SECS));
        let mut interface_interval = tokio::time::interval(Duration::from_secs(3));
        probe_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        interface_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                _ = kick.notified() => {
                    probe_once(&reachability, &readers).await;
                }
                _ = probe_interval.tick() => {
                    probe_once(&reachability, &readers).await;
                }
                _ = interface_interval.tick() => {
                    let next = interface_fingerprint();
                    if next != interfaces {
                        interfaces = next;
                        reachability.invalidate();
                        kick.notify_one();
                    }
                }
            }
        }
    })
}

/// Called when a live request fails to connect to LARM: record the failure and probe now,
/// so the next request already sees the new state instead of waiting for the interval.
pub(crate) fn report_larm_connect_failure(state: &AppState) {
    state.reachability.record(false, Instant::now());
    state.reachability_kick.notify_one();
}

async fn probe_once(
    reachability: &super::reachability::ReachabilityState,
    readers: &crate::persistence::SqliteReaders,
) {
    let address = readers.read(|connection| Ok(load_model_providers(connection)?.harness.address));
    let Ok(address) = address else {
        reachability.invalidate();
        return;
    };
    let Some(base) = harness_base_url(&address) else {
        reachability.invalidate();
        eprintln!("larm reachability probe skipped: no valid harness address");
        return;
    };
    let timeout = Duration::from_millis(PROBE_TIMEOUT_MS);
    // One retry: a single dropped packet (Wi-Fi roaming, VPN switch) must not send a request to the cloud.
    let ok = reachable(base.clone(), timeout).await || reachable(base, timeout).await;
    reachability.record(ok, Instant::now());
    eprintln!("larm reachability probe ok={ok}");
}

fn harness_base_url(address: &str) -> Option<url::Url> {
    let url = url::Url::parse(address.trim()).ok()?;
    (matches!(url.scheme(), "http" | "https") && url.host_str().is_some()).then_some(url)
}

/// Any HTTP response (even an error status) proves the harness host is on the network.
async fn reachable(base: url::Url, timeout: Duration) -> bool {
    let Ok(client) = reqwest::Client::builder()
        .connect_timeout(timeout)
        .timeout(timeout)
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .build()
    else {
        return false;
    };
    client.get(base).send().await.is_ok()
}

fn interface_fingerprint() -> u64 {
    let mut pairs = if_addrs::get_if_addrs()
        .unwrap_or_default()
        .into_iter()
        .map(|interface| (interface.name.clone(), interface.ip().to_string()))
        .collect::<Vec<_>>();
    pairs.sort();
    let mut hasher = DefaultHasher::new();
    pairs.hash(&mut hasher);
    hasher.finish()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;

    fn serve(status: &'static str) -> (url::Url, std::thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("listener");
        let address = listener.local_addr().expect("address");
        let server = std::thread::spawn(move || {
            let Ok((mut stream, _)) = listener.accept() else {
                return;
            };
            let mut buffer = [0_u8; 1024];
            let _ = stream.read(&mut buffer);
            let response =
                format!("HTTP/1.1 {status}\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{{}}");
            let _ = stream.write_all(response.as_bytes());
        });
        (
            url::Url::parse(&format!("http://{address}/")).expect("url"),
            server,
        )
    }

    #[tokio::test]
    async fn any_http_response_is_reachable_and_a_refused_connection_is_not() {
        for status in ["200 OK", "401 Unauthorized"] {
            let (url, server) = serve(status);
            assert!(reachable(url, Duration::from_secs(2)).await, "{status}");
            server.join().expect("server");
        }
        let listener = TcpListener::bind("127.0.0.1:0").expect("listener");
        let address = listener.local_addr().expect("address");
        drop(listener);
        let refused = url::Url::parse(&format!("http://{address}/")).expect("url");
        assert!(!reachable(refused, Duration::from_millis(200)).await);
    }

    #[test]
    fn only_http_urls_with_a_host_are_probed() {
        assert!(harness_base_url("http://192.0.2.1:7001").is_some());
        assert!(harness_base_url("https://larm.example.test/").is_some());
        assert!(harness_base_url("").is_none());
        assert!(harness_base_url("ftp://192.0.2.1").is_none());
    }
}
