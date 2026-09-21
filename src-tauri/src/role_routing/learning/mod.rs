// These offline-learning stages are exercised by their gate tests before they are wired into
// the online runtime. Keep dead-code suppression scoped to the production build of each stage.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) mod artifact;
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) mod evaluate;
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) mod export;
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) mod invalidation;
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) mod repository;
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) mod scheduler;
pub(crate) mod schema;
