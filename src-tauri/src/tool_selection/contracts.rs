//! Configuration, request context, error envelope, semantic labels and limits for the
//! tool-selection D0–D3 milestone.
//!
//! Everything here is pure data plus parsing. No SQL and no inference live in this module so the
//! same types can be used by tests, the gateway and the L-Lang backend adapter.

use serde::{Deserialize, Serialize};
use std::fmt;
use std::path::{Path, PathBuf};
include!("contracts.d/01.rs");
include!("contracts.d/02.rs");
