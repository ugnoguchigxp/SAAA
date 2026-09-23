pub(super) use crate::{
    ipc_contract::RuntimeEvent,
    voice::session::{selected_tts_route, TtsRoute},
    AppState, RunCancellation,
};
use futures_util::{stream::FuturesUnordered, StreamExt};
pub(super) use std::{
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
#[path = "runtime/streaming_speech_runtime.rs"]
mod streaming_speech_runtime;
#[path = "runtime/render_session_inner.rs"]
mod render_session_inner;
pub(crate) use streaming_speech_runtime::{StreamingSpeechRuntime, AppendOutcome};
pub(super) use streaming_speech_runtime::{
    MAX_QUEUED_CHUNKS, MAX_RENDER_CONCURRENCY, MAX_READY_CHUNKS, MAX_READY_AUDIO_BYTES,
    MAX_READY_AUDIO_MS, SpeechSession, SpeechWork, RenderSessionContext, RenderedChunk,
    RenderFuture, PlaybackTask,
};
pub(super) use render_session_inner::{
    render_session_inner, finalize_rendered_chunk, adaptive_render_concurrency, render_slots_used,
    wave_duration_ms, wait_for_child, render_future, resolve_render_route,
};
#[cfg(test)]
#[path = "runtime/tests.rs"]
mod tests;
