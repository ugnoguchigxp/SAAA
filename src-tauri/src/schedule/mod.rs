pub(crate) mod calendar;
pub(crate) mod commands;
pub mod contracts;
pub(crate) mod decide;
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) mod forget;
#[cfg_attr(not(test), allow(dead_code))]
mod handle;
pub(crate) mod hold;
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) mod ledger;
pub(crate) mod notify;
pub(crate) mod runtime;
pub(crate) mod schema;
pub(crate) mod tick;
pub(crate) use handle::Handle;
pub(crate) use schema::migrate;
pub(crate) use tick::start_loop;
pub(crate) fn hydrate(state: &crate::AppState) {
    if let Ok(s) = state.sqlite_writer.read_serialized(runtime::load) {
        state.schedule.set_enabled(s.enabled);
        state
            .schedule
            .set_calendar_ready(s.calendar_enabled && s.calendar_id.is_some());
    }
    let allow_legacy = calendar::auth::legacy_entry_is_unambiguous(state);
    if let Ok(Some(token)) = calendar::auth::load_refresh(&state.schedule, allow_legacy) {
        state.schedule.set_refresh(&token);
    }
}
#[cfg(test)]
mod tests;
