use crate::voice_text::text_for_speech;
use unicode_segmentation::UnicodeSegmentation;
#[path = "chunker/select_reason.rs"]
mod select_reason;
#[path = "chunker/is_weak_boundary.rs"]
mod is_weak_boundary;
pub(crate) use select_reason::{FIRST_MIN, STEADY_MIN, TARGET, HARD_MAX, MAX_SOURCE_CHARS, SelectReason, SpeechChunk, AccumulatorError, SentenceAccumulator};
use is_weak_boundary::{is_weak_boundary, is_safe_boundary};
