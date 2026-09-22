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
include!("config.d/01.rs");
include!("config.d/02.rs");
