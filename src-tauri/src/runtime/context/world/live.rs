//! Live World frame state for one composed turn.
//!
//! `WorldLive` owns the prepared frame used for revalidation, the two renderings of the combined
//! personal block (with and without World) and the post-dispatch receipt binding. Keeping this out
//! of `turn.rs` separates the compose logic from the per-request runtime state.

use super::super::generation::GenerationHandle;
use super::super::source::Candidate;
use super::source::WORLD_KIND;
use crate::memory::personal_state::world::runtime_frame::{PreparedWorldFrame, WorldFrameService};
use std::sync::{Arc, Mutex};

pub(crate) struct WorldReceipt {
    pub(crate) service: Arc<WorldFrameService>,
    pub(crate) prepared: PreparedWorldFrame,
    pub(crate) dispatched_at_ms: i64,
}

/// The two renderings of the combined personal block: `with_world` is what the composed body
/// currently contains, `without_world` is the same block with the World candidate removed (or
/// `None` when the World was the only selected candidate, so the block disappears entirely).
#[derive(Clone, Debug)]
pub(crate) struct WorldBlocks {
    pub(crate) with_world: String,
    pub(crate) without_world: Option<String>,
}

enum WorldFrame {
    Live {
        service: Arc<WorldFrameService>,
        prepared: Mutex<Option<PreparedWorldFrame>>,
    },
    /// Test-only fixed validity, so the send-body regression test does not need a live service.
    #[cfg(test)]
    Fixed(bool),
}

pub(crate) struct WorldLive {
    frame: WorldFrame,
    blocks: Mutex<Option<WorldBlocks>>,
}

impl WorldLive {
    pub(crate) fn live(
        service: Arc<WorldFrameService>,
        prepared: PreparedWorldFrame,
        blocks: Option<WorldBlocks>,
    ) -> Self {
        Self {
            frame: WorldFrame::Live {
                service,
                prepared: Mutex::new(Some(prepared)),
            },
            blocks: Mutex::new(blocks),
        }
    }

    /// Re-checks the currently prepared frame once. A failed check clears the prepared frame, so
    /// every later call also reports invalid and the send body and the record cannot diverge.
    pub(crate) fn revalidate_current(&self) -> bool {
        match &self.frame {
            WorldFrame::Live { service, prepared } => {
                let frame = {
                    let guard = prepared
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    let Some(frame) = guard.as_ref() else {
                        return false;
                    };
                    frame.clone()
                };
                match service.revalidate_frame(&frame) {
                    Ok(saaa_personal_state_core::world::runtime_frame::FrameValidity::Current) => {
                        true
                    }
                    _ => {
                        *prepared
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
                        false
                    }
                }
            }
            #[cfg(test)]
            WorldFrame::Fixed(valid) => *valid,
        }
    }

    /// The with-World and without-World renderings of the combined block, if the frame was
    /// prepared with a block.
    pub(crate) fn blocks(&self) -> Option<WorldBlocks> {
        self.blocks
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    #[cfg(test)]
    pub(crate) fn for_test(valid: bool, with_world: &str, without_world: Option<&str>) -> Self {
        Self {
            frame: WorldFrame::Fixed(valid),
            blocks: Mutex::new(Some(WorldBlocks {
                with_world: with_world.to_string(),
                without_world: without_world.map(str::to_string),
            })),
        }
    }

    #[cfg(test)]
    pub(crate) fn without_blocks() -> Self {
        Self {
            frame: WorldFrame::Fixed(true),
            blocks: Mutex::new(None),
        }
    }

    pub(crate) fn bind(&self, generation: &GenerationHandle) {
        match &self.frame {
            WorldFrame::Live { service, prepared } => {
                let guard = prepared
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                if let Some(prepared) = guard.as_ref() {
                    generation.attach_world(WorldReceipt {
                        service: service.clone(),
                        prepared: prepared.clone(),
                        dispatched_at_ms: crate::memory::personal_state::now(),
                    });
                }
            }
            #[cfg(test)]
            WorldFrame::Fixed(_) => {}
        }
    }
}

pub(crate) fn observe_receipt(receipt: &WorldReceipt) -> &'static str {
    match receipt.service.revalidate_frame(&receipt.prepared) {
        Ok(saaa_personal_state_core::world::runtime_frame::FrameValidity::Current) => "current",
        Ok(saaa_personal_state_core::world::runtime_frame::FrameValidity::Expired) => {
            "expired-after-dispatch"
        }
        Ok(saaa_personal_state_core::world::runtime_frame::FrameValidity::Changed) => {
            "changed-after-dispatch"
        }
        Ok(saaa_personal_state_core::world::runtime_frame::FrameValidity::ScopeDenied) => {
            "scope-denied-after-dispatch"
        }
        Ok(saaa_personal_state_core::world::runtime_frame::FrameValidity::Unavailable) | Err(_) => {
            "unavailable-after-dispatch"
        }
    }
}

/// The candidates the record should contain for one request. The World candidate is only kept when
/// the caller decided to include it in the sent body; a World-free history always drops it.
pub(crate) fn for_record<'a>(
    selected: &'a [Candidate],
    omitted: &'a [Candidate],
    include_world: bool,
) -> (Vec<&'a Candidate>, Vec<&'a Candidate>) {
    let mut kept: Vec<&Candidate> = selected
        .iter()
        .filter(|candidate| candidate.source_kind != WORLD_KIND)
        .collect();
    let omitted: Vec<&Candidate> = omitted
        .iter()
        .filter(|candidate| candidate.source_kind != WORLD_KIND)
        .collect();
    if include_world {
        if let Some(candidate) = selected
            .iter()
            .find(|candidate| candidate.source_kind == WORLD_KIND)
        {
            kept.push(candidate);
        }
    }
    (kept, omitted)
}
