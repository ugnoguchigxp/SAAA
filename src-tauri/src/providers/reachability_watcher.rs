//! Background observation of the LAN Provider Harness. Selection reads only the snapshot.
use super::reachability::{PROBE_INTERVAL_SECS, PROBE_TIMEOUT_MS};
use crate::persistence::load_model_providers;
use crate::{AppState, ModelProviderSettings, DYNAMIC_LAN_PROVIDER_ID};
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::Arc;
use std::time::{Duration, Instant};

pub(crate) fn spawn(state: &AppState) -> tokio::task::JoinHandle<()> {
    let reachability = Arc::clone(&state.reachability);
    let kick = Arc::clone(&state.reachability_kick);
    let readers = state.sqlite_readers.clone();
    tokio::spawn(async move {
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

async fn probe_once(
    reachability: &super::reachability::ReachabilityState,
    readers: &crate::persistence::SqliteReaders,
) {
    let host = readers.read(|connection| {
        let providers = load_model_providers(connection)?;
        Ok(enabled_harness_host(&providers.providers))
    });
    let Ok(host) = host else {
        reachability.invalidate();
        return;
    };
    let Some(host) = host else {
        reachability.invalidate();
        eprintln!("dynamic_lan reachability probe skipped: no enabled provider");
        return;
    };
    let ok =
        super::dynamic_lan::probe::reachable(&host, Duration::from_millis(PROBE_TIMEOUT_MS)).await;
    reachability.record(ok, Instant::now());
    eprintln!("dynamic_lan reachability probe ok={ok}");
}

fn enabled_harness_host(providers: &[ModelProviderSettings]) -> Option<String> {
    let dynamic = providers.iter().filter_map(|provider| match provider {
        ModelProviderSettings::DynamicLan(settings) if settings.enabled => Some(settings),
        _ => None,
    });
    let mut fallback = None;
    for settings in dynamic {
        if settings.id == DYNAMIC_LAN_PROVIDER_ID {
            return Some(settings.host.clone());
        }
        if fallback.is_none() {
            fallback = Some(settings.host.clone());
        }
    }
    fallback
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
