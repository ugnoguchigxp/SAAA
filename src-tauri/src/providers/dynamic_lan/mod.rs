use crate::RunCancellation;
use reqwest::{header::HeaderValue, Method, StatusCode};
use serde::Deserialize;
use serde_json::json;
use std::{sync::Arc, time::Duration};
use url::Url;
use uuid::Uuid;
use zeroize::Zeroizing;
mod auth;
pub(crate) mod credential;
mod http;
pub(crate) mod probe;
mod profile_catalog;
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
    include!("tests/fixtures_and_contracts.rs");
    include!("tests/resolution_tests.rs");
    include!("tests/selector_tests.rs");
    include!("tests/lifecycle_tests.rs");
}
