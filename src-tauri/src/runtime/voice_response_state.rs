use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

#[derive(Clone, Default)]
pub(crate) struct HubState {
    enabled: bool,
    completion: Arc<Mutex<HashMap<String, String>>>,
    sequence: Arc<Mutex<HashMap<String, u8>>>,
}

impl HubState {
    pub(crate) fn enable(&mut self, requested: bool, streaming_speech: bool) {
        self.enabled = requested && streaming_speech;
    }

    pub(crate) fn enabled(&self) -> bool {
        self.enabled
    }

    pub(crate) fn set_completion(&self, run_id: &str, text: String) {
        if !self.enabled || text.trim().is_empty() {
            return;
        }
        self.completion
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(run_id.to_string(), text);
    }

    pub(crate) fn take_completion(&self, run_id: &str) -> Option<String> {
        self.completion
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(run_id)
    }

    pub(crate) fn clear(&self, run_id: &str) {
        self.take_completion(run_id);
        self.sequence
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(run_id);
    }

    pub(super) fn queue_in_sequence(
        &self,
        run_id: &str,
        kind: crate::larm_voice::ResponseKind,
        queue: impl FnOnce() -> Result<(), String>,
    ) -> Result<(), String> {
        let mut sequence = self
            .sequence
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let next = kind.sequence();
        if sequence.get(run_id).is_some_and(|current| *current > next) {
            return Ok(());
        }
        queue()?;
        sequence.insert(run_id.to_string(), next);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requires_an_active_streaming_speech_route() {
        let mut state = HubState::default();
        state.enable(true, false);
        assert!(!state.enabled());
        state.enable(true, true);
        assert!(state.enabled());
    }
}
