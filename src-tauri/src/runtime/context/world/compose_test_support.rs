//! Offline composition fixture entry point.
#[path = "compose_fixture.rs"]
mod fixture;
pub(crate) use fixture::{compose_fixture, ComposedFixture};
