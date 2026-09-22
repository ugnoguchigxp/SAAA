#![cfg(test)]

//! Deterministic World fixtures for adapter tests. No model, no network.

use crate::memory::personal_state::{encode, now, sources, store};
use crate::persistence::sqlite::SqliteWriter;
use rusqlite::{params, Connection};
use saaa_personal_state_core::world::model_v2::EntityKindV2;
use saaa_personal_state_core::world::versioned::{decode_versioned, VersionedWorldPayload};
use saaa_personal_state_core::world::*;
use saaa_personal_state_core::*;
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
include!("test_support.d/01.rs");
include!("test_support.d/02.rs");
