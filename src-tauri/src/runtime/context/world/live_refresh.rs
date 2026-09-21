//! Refresh and claim acceptance for a new generation.
use super::*;
impl WorldLive {
    /// Refresh for a new tool-followup generation. Existing legacy fixtures keep their
    /// initial-only contract; production services have source-backed refresh enabled.
    pub(crate) fn refresh_blocks(&self) -> Result<Option<(WorldBlocks, WorldBlocks)>, String> {
        let (service, prepared, seed) = match &self.frame {
            WorldFrame::Live {
                service,
                prepared,
                seed,
            } => (service, prepared, seed),
            #[cfg(test)]
            WorldFrame::Fixed(_) => return Ok(None),
        };
        if !service.sources_enabled() {
            return Ok(None);
        }
        let mut seed = seed.lock().map_err(|_| "World seed unavailable")?;
        let old_frame = seed.clone();
        let new_frame = service.refresh(&seed).map_err(|e| e.code().to_string())?;
        let old_content = super::render::render_world_frame_explicit(old_frame.frame())
            .map_err(|_| "World render failed")?;
        let new_content = super::render::render_world_frame_explicit(new_frame.frame())
            .map_err(|_| "World render failed")?;
        let mut blocks = self.blocks.lock().map_err(|_| "World blocks unavailable")?;
        let Some(old) = blocks.as_ref().cloned() else {
            return Ok(None);
        };
        if old.with_world.matches(&old_content).count() != 1 {
            return Err("World block provenance mismatch".into());
        }
        let new = WorldBlocks {
            with_world: old.with_world.replacen(&old_content, &new_content, 1),
            without_world: old.without_world.clone(),
        };
        *seed = new_frame.clone();
        *prepared.lock().map_err(|_| "World frame unavailable")? = Some(new_frame);
        *blocks = Some(new.clone());
        Ok(Some((old, new)))
    }

    pub(crate) fn refresh_history(
        &self,
        history: &mut [ConversationMessage],
    ) -> Result<bool, String> {
        let Some((old, new)) = self.refresh_blocks()? else {
            return Ok(false);
        };
        let mut replaced = 0;
        for message in history {
            if message.role == "assistant" && message.content == old.with_world {
                message.content = new.with_world.clone();
                replaced += 1;
            }
        }
        if replaced != 1 {
            return Err("World history provenance mismatch".into());
        }
        Ok(true)
    }

    pub(crate) fn accept_claims(&self, raw: &str, question: &str) -> Result<String, String> {
        match &self.frame {
            WorldFrame::Live {
                service, prepared, ..
            } => {
                let prior = prepared
                    .lock()
                    .map_err(|_| "World frame unavailable")?
                    .clone()
                    .ok_or("World frame invalidated")?;
                if service
                    .validate_result(&prior)
                    .map_err(|e| e.code().to_string())?
                    != saaa_personal_state_core::world::runtime_frame::FrameValidity::Current
                {
                    return Err("state-claim-source-changed".into());
                }
                super::state_claim::render_for_query(raw, prior.frame(), question)
            }
            #[cfg(test)]
            WorldFrame::Fixed(_) => Err("state-claim-unavailable".into()),
        }
    }

    pub(crate) fn current_candidate(&self) -> Option<Candidate> {
        match &self.frame {
            WorldFrame::Live { prepared, .. } => {
                let guard = prepared.lock().ok()?;
                let frame = guard.as_ref()?.frame();
                let content = super::render::render_world_frame_explicit(frame).ok()?;
                Some(super::source::frame_candidate(frame, &content, WORLD_KIND))
            }
            #[cfg(test)]
            WorldFrame::Fixed(_) => None,
        }
    }
}
