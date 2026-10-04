#[path = "generation_validation.rs"]
mod validation;
use crate::persistence::SqliteWriter;
#[cfg(any(test, feature = "offline-contracts"))]
use crate::AppState;
use crate::{database_error, new_id, now_iso};
use rusqlite::{params, Connection, OptionalExtension};
use sha2::{Digest, Sha256};
use std::sync::{Arc, Mutex};
use validation::{digest, resolve_provider, validate_dependencies};
#[path = "generation/final_wire_size.rs"]
mod final_wire_size;
#[path = "generation_test_helpers.rs"]
mod test_helpers;
#[cfg(any(test, feature = "offline-contracts"))]
pub(crate) use self::final_wire_size::{begin, begin_with_current_instruction, final_wire_size};
#[cfg(test)]
pub(crate) use final_wire_size::assert_two_round_tool_manifest;
#[cfg(test)]
pub(crate) use final_wire_size::begin_direct_dispatched;
#[cfg(any(test, feature = "offline-contracts"))]
pub(crate) use final_wire_size::FinalWireSize;
pub(crate) use final_wire_size::{
    begin_with_writer, BeginGeneration, GenerationHandle, MAX_PROVIDER_CONTEXT_WIRE_BYTES,
};
#[cfg(any(test, feature = "offline-contracts"))]
pub(crate) use final_wire_size::{finish_result, record_red, MAX_PROVIDER_REQUEST_BYTES};
#[cfg(test)]
#[path = "generation/tests/mod.rs"]
mod tests;
