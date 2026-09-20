pub(crate) mod commands;
pub(crate) mod contracts;
pub(crate) mod driver;
mod reduce;
mod report;
mod repository;
pub(crate) mod schema;
pub(crate) mod tools;

pub(crate) use reduce::dispatch_scheduled;
#[cfg(test)]
pub(crate) use reduce::inspect_coding_transition;
pub(crate) use reduce::on_user_message;
pub(crate) use report::{flush_all_held_reports, flush_held_reports};
pub(crate) use repository::forget_source;

pub(crate) const START_TRIGGER: &str = "テストを確認して";
pub(crate) const CONTINUE_TRIGGER: &str = "続きを";
pub(crate) const START_REQUEST: &str =
    "Inspect failing tests in this workspace. Read logs and report causes. Do not change files.";
pub(crate) const DEDUPE_SUFFIX: &str = "fixture:test-failure";

#[cfg(test)]
mod tests {
    include!("tests.rs");
}
