pub(crate) mod auth;
pub(crate) mod client;
pub(crate) mod encode;
pub(crate) mod oauth;
pub(crate) mod observe;
pub(crate) mod projection;
pub(crate) mod reconcile;

pub(crate) fn rfc3339(ms: i64) -> String {
    chrono::DateTime::from_timestamp_millis(ms)
        .unwrap_or_else(|| chrono::DateTime::from_timestamp(0, 0).expect("epoch"))
        .to_rfc3339()
}
