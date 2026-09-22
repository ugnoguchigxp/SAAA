#[path = "generation_validation.rs"]
mod validation;
use crate::persistence::SqliteWriter;
use crate::{database_error, new_id, now_iso, AppState};
use rusqlite::{params, Connection, OptionalExtension};
use sha2::{Digest, Sha256};
use std::sync::{Arc, Mutex};
use validation::{digest, resolve_provider, validate_dependencies};
#[path = "generation_test_helpers.rs"]
mod test_helpers;
include!("generation.d/01.rs");
#[cfg(test)]
mod tests {
    include!("generation.d/02.rs");
    include!("generation.d/03.rs");
}
