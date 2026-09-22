use super::speaker::SpeakerExtractor;
use crate::persistence::{SqliteReaders, SqliteWriter};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::Mutex,
};
use zeroize::Zeroizing;
mod codec;
mod enrollment;
pub(crate) mod streaming_verifier;
use codec::*;
pub use codec::{migrate_v10_to_v11, migrate_v14_to_v15, reconcile_voice_profile_storage};
include!("mod.d/01.rs");
#[cfg(test)]
mod tests {
    include!("mod.d/02.rs");
    include!("mod.d/03.rs");
}
