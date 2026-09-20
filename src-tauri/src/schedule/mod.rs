pub(crate) mod calendar;
pub(crate) mod commands;
pub mod contracts;
pub(crate) mod decide;
pub(crate) mod forget;
mod handle;
pub(crate) mod hold;
pub(crate) mod ledger;
pub(crate) mod notify;
pub(crate) mod runtime;
pub(crate) mod schema;
mod tick;
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
}
#[cfg(test)]
mod tests;
