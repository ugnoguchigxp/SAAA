use crate::voice_text::text_for_speech;
use unicode_segmentation::UnicodeSegmentation;
#[path = "chunker/is_weak_boundary.rs"]
mod is_weak_boundary;
#[path = "chunker/select_reason.rs"]
mod select_reason;
use is_weak_boundary::{is_safe_boundary, is_weak_boundary};
pub(crate) use select_reason::{
    AccumulatorError, SelectReason, SentenceAccumulator, SpeechChunk, FIRST_MIN, HARD_MAX,
    MAX_SOURCE_CHARS, STEADY_MIN, TARGET,
};
