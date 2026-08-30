use std::{
    collections::VecDeque,
    sync::{Mutex, OnceLock},
    time::{Duration, Instant},
};

use super::HarnessDescriptor;

const CACHE_CAPACITY: usize = 8;
const CACHE_TTL: Duration = Duration::from_secs(30);

struct Entry {
    address: String,
    descriptor: HarnessDescriptor,
    inserted_at: Instant,
}

fn entries() -> &'static Mutex<VecDeque<Entry>> {
    static CACHE: OnceLock<Mutex<VecDeque<Entry>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(VecDeque::new()))
}

pub(super) fn get(address: &url::Url) -> Option<HarnessDescriptor> {
    let mut entries = entries()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    entries.retain(|entry| entry.inserted_at.elapsed() < CACHE_TTL);
    let index = entries
        .iter()
        .position(|entry| entry.address == address.as_str())?;
    let entry = entries.remove(index)?;
    let descriptor = entry.descriptor.clone();
    entries.push_back(entry);
    Some(descriptor)
}

pub(super) fn put(address: &url::Url, descriptor: HarnessDescriptor) {
    let mut entries = entries()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    entries.retain(|entry| entry.address != address.as_str());
    entries.push_back(Entry {
        address: address.as_str().to_string(),
        descriptor,
        inserted_at: Instant::now(),
    });
    while entries.len() > CACHE_CAPACITY {
        entries.pop_front();
    }
}

pub(super) fn clear() {
    entries()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clear();
}
