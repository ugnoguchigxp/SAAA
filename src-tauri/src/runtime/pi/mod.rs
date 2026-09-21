// Delegated profiles and recipes are offline-gated runtime contracts.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) mod delegated_profile;
pub(crate) mod process;
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) mod recipe_runner;
pub(crate) mod runner;
pub(crate) mod session_reader;
#[cfg(test)]
mod tests;
