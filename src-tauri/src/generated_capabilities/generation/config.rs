//! Generation configuration and the host-registered request catalog (plan 12.1, 12.2).
//!
//! `SAAA_LLANG_GENERATION_CONFIG` points at an absolute JSON file. Unknown fields are rejected
//! and `enabled` may not be omitted. The request file is read separately and its bytes are hashed
//! at load time so a later edit to a registered request/suite/metadata is detected.

use super::super::contracts::sha256_hex;
use super::super::errors::CapabilityError;
use super::super::errors::{CapabilityErrorCode, CapabilityResult};
use super::contracts::{encode_error, GenerationErrorCode, GENERATION_FORMAT_VERSION};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::{Path, PathBuf},
};
#[path = "config/generation_config.rs"]
mod generation_config;
pub use generation_config::{GENERATION_CONFIG_ENV, KIT_FORMAT_VERSION, REQUESTS_FORMAT_VERSION, MAX_CONFIG_BYTES, MAX_REQUESTS_BYTES, MAX_REQUEST_ENTRIES, MAX_REQUEST_FIELDS, MIN_REQUEST_FIELDS, MAX_PURPOSE_BYTES, GenerationConfig, RegisteredRequest, RequestScope, load_config, enabled_config, from_environment, load_requests};
#[cfg(test)]
#[path = "config/tests.rs"]
mod tests;
