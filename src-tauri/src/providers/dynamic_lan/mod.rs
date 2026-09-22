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
include!("mod.d/01.rs");
include!("mod.d/02.rs");
#[cfg(test)]
mod tests {
    include!("mod.d/03.rs");
    include!("mod.d/04.rs");
    include!("mod.d/05.rs");
}
