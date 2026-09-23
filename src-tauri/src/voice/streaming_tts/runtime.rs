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
#[path = "runtime/render_session_inner.rs"]
mod render_session_inner;
#[path = "runtime/streaming_speech_runtime.rs"]
mod streaming_speech_runtime;
pub(super) use render_session_inner::{
    adaptive_render_concurrency, finalize_rendered_chunk, render_future, render_session_inner,
    render_slots_used, resolve_render_route, wait_for_child, wave_duration_ms,
};
pub(crate) use streaming_speech_runtime::{AppendOutcome, StreamingSpeechRuntime};
pub(super) use streaming_speech_runtime::{
    PlaybackTask, RenderFuture, RenderSessionContext, RenderedChunk, SpeechSession, SpeechWork,
    MAX_QUEUED_CHUNKS, MAX_READY_AUDIO_BYTES, MAX_READY_AUDIO_MS, MAX_READY_CHUNKS,
    MAX_RENDER_CONCURRENCY,
};
#[cfg(test)]
#[path = "runtime/tests.rs"]
mod tests;
