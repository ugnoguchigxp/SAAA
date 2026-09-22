#![cfg(test)]

use super::*;
use crate::runtime::event_hub::RuntimeEventSender;
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
#[path = "coding_tests.rs"]
mod coding_tests;
#[path = "generated_tools_flags_tests.rs"]
mod generated_tools_flags_tests;
#[path = "generated_tools_tests.rs"]
mod generated_tools_tests;
#[path = "json_tool_tests.rs"]
mod json_tool_tests;
include!("tests.d/01.rs");
include!("tests.d/02.rs");
