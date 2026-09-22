use crate::{
    ipc_contract::RuntimeEvent,
    voice::session::{selected_tts_route, TtsRoute},
    AppState, RunCancellation,
};
use futures_util::{stream::FuturesUnordered, StreamExt};
use std::{
    collections::{BTreeMap, HashMap, VecDeque},
    fs,
    future::Future,
    io::Read,
    path::{Path, PathBuf},
    pin::Pin,
    process::Child,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use tokio::sync::{mpsc, watch};
#[path = "fallback.rs"]
mod fallback;
use super::chunker::{SelectReason, SentenceAccumulator, MAX_SOURCE_CHARS};
include!("runtime.d/01.rs");
include!("runtime.d/02.rs");
include!("runtime.d/03.rs");
