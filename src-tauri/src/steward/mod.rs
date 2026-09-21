pub(crate) mod admission;
pub(crate) mod authority;
pub(crate) mod budget;
pub(crate) mod commands;
pub(crate) mod contracts;
pub(crate) mod dispatch;
pub(crate) mod driver;
pub(crate) mod evidence;
pub(crate) mod execution_contracts;
pub(crate) mod faults;
pub(crate) mod intake;
pub(crate) mod invalidation;
pub(crate) mod outbox;
pub(crate) mod plans;
pub(crate) mod pump;
pub(crate) mod queue;
pub(crate) mod recipes;
mod reduce;
pub(crate) mod report;
pub(crate) mod report_content;
mod repository;
pub(crate) mod request_intent;
pub(crate) mod schema;
pub(crate) mod schema_execution;
pub(crate) mod tools;
pub(crate) mod verifier;
pub(crate) mod views;

pub(crate) use dispatch::dispatch_scheduled;
pub(crate) use invalidation::forget_source;
#[cfg(test)]
pub(crate) use reduce::inspect_coding_transition;
pub(crate) use reduce::on_user_message;
pub(crate) use report::{flush_all_held_reports, flush_held_reports};

pub(crate) const START_TRIGGER: &str = "テストを確認して";
pub(crate) const CONTINUE_TRIGGER: &str = "続きを";
pub(crate) const START_REQUEST: &str =
    "Inspect failing tests in this workspace. Read logs and report causes. Do not change files.";
pub(crate) const DEDUPE_SUFFIX: &str = "fixture:test-failure";

#[cfg(test)]
mod tests {
    include!("tests.rs");
}

#[cfg(test)]
mod dwr_tests {
    include!("tests/dwr.rs");
}

#[cfg(test)]
mod migration_tests {
    include!("tests/migration.rs");
}

#[cfg(test)]
mod latency_tests {
    include!("tests/latency.rs");
}

#[cfg(test)]
mod rf5_tests {
    include!("tests/rf5.rs");
}
