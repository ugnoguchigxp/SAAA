//! Conservative, local policy adaptation.
//!
//! This is deliberately a *re-ordering* layer: callers provide candidates that have already
//! passed their domain's permission and capability checks.  An adaptive policy can therefore
//! never introduce a tool, recipe, plan step, or notification channel.
use chrono::{Datelike, Timelike};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::sync::Arc;
include!("adaptive_improvement.d/01.rs");
include!("adaptive_improvement.d/02.rs");
include!("adaptive_improvement.d/03.rs");
