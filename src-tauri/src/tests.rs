#![cfg(test)]

use super::*;
use crate::persistence::save_settings_documents_to_connection;
use crate::test_support::*;
use rusqlite::params;
use serde_json::{json, Value};
use std::{
    io::Read,
    process::{Command, Stdio},
    sync::mpsc,
    thread,
};
include!("tests.d/01.rs");
include!("tests.d/02.rs");
include!("tests.d/03.rs");
include!("tests.d/04.rs");
include!("tests.d/05.rs");
include!("tests.d/06.rs");
include!("tests.d/07.rs");
include!("tests.d/08.rs");
