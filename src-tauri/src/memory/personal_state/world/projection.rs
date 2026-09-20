//! World projection rebuild entry (WM-06). The versioned implementation lives in
//! `projection_v2.rs`; v1 payloads normalize to the v2 view there and the meta
//! row records `projection_version` 1 or 2.

pub fn rebuild(
    c: &rusqlite::Connection,
    ledger: &saaa_personal_state_core::Ledger,
    now: i64,
) -> Result<(), String> {
    super::projection_v2::rebuild_v2(c, ledger, now)
}
