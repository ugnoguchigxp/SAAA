//! Local HTTP host for the media lab. It does not open the desktop database.

mod auth;
mod fixture;
mod http;
mod owner;
mod store;

#[cfg(test)]
mod acceptance;

pub use auth::LabAuth;
pub use http::{authorized_host, listen_host, open_service, router, LabHttp, RunningLab};
pub use owner::OwnerGuard;
pub use store::{open_lab, LabConfig, LabOpen};
