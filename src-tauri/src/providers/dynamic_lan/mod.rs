use crate::RunCancellation;
use reqwest::{header::HeaderValue, Method, StatusCode};
use serde::Deserialize;
use serde_json::json;
#[cfg(any(test, feature = "offline-contracts"))]
use serde_json::Value;
use std::{sync::Arc, time::Duration};
use url::Url;
use uuid::Uuid;
use zeroize::Zeroizing;
mod auth;
pub(crate) mod credential;
mod http;
pub(crate) mod probe;
mod profile_catalog;
mod profile_contract;
mod urls;
mod validate;
use auth::*;
use http::*;
use profile_catalog::*;
use urls::*;
pub(crate) use urls::{control_base_url, url_is_local};
use validate::*;
include!("connection_types.rs");
include!("connection_lifecycle.rs");
include!("connection_runtime.rs");
#[cfg(test)]
mod tests {
    mod cold_services;
    include!("tests/fixtures_and_contracts.rs");
    include!("tests/resolution_tests.rs");
    #[path = "selector_tests.rs"]
    mod selector_tests;
    use selector_tests::spawn_json_server;
    include!("tests/lifecycle_tests.rs");
}
