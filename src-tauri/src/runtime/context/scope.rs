use crate::{database_error, now_iso, StartTurnInput};
use rusqlite::{params, Connection, OptionalExtension};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
include!("scope.d/01.rs");
include!("scope.d/02.rs");
