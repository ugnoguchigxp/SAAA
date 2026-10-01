use std::collections::HashMap;
use std::sync::Mutex;

#[derive(Clone, Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Proposal {
    pub(crate) lookup_id: String,
    pub(crate) conversation_id: String,
    pub(crate) run_id: String,
    pub(crate) written: String,
    pub(crate) current_spoken: Option<String>,
    pub(crate) proposed_spoken: Option<String>,
}

#[derive(Default)]
struct Slot {
    lookup: Option<Proposal>,
    pending: Option<(Proposal, bool)>,
}

#[derive(Default)]
pub(crate) struct Proposals(Mutex<HashMap<String, Slot>>);

impl Proposals {
    pub(crate) fn issue(
        &self,
        conversation: &str,
        run: &str,
        written: String,
        current: Option<String>,
        proposed: Option<String>,
    ) -> Proposal {
        let proposal = Proposal {
            lookup_id: uuid::Uuid::new_v4().to_string(),
            conversation_id: conversation.into(),
            run_id: run.into(),
            written,
            current_spoken: current,
            proposed_spoken: proposed,
        };
        let mut slots = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        // This store contains transient confirmations, never dictionary contents.
        if !slots.contains_key(conversation) && slots.len() >= 32 {
            slots.clear();
        }
        let slot = slots.entry(conversation.into()).or_default();
        slot.lookup = Some(proposal.clone());
        slot.pending = None;
        proposal
    }

    pub(crate) fn get(
        &self,
        conversation: &str,
        run: &str,
        id: &str,
        confirmation: bool,
    ) -> Option<Proposal> {
        let slots = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let slot = slots.get(conversation)?;
        if confirmation {
            let (proposal, published) = slot.pending.as_ref()?;
            (*published && proposal.lookup_id == id && proposal.run_id != run)
                .then(|| proposal.clone())
        } else {
            let proposal = slot.lookup.as_ref()?;
            (proposal.lookup_id == id && proposal.run_id == run).then(|| proposal.clone())
        }
    }

    pub(crate) fn require_confirmation(&self, proposal: &Proposal) {
        let mut slots = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(slot) = slots.get_mut(&proposal.conversation_id) {
            if slot
                .lookup
                .as_ref()
                .is_some_and(|p| p.lookup_id == proposal.lookup_id)
            {
                slot.pending = Some((proposal.clone(), false));
            }
        }
    }

    pub(crate) fn pending(&self, conversation: &str) -> Option<Proposal> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(conversation)
            .and_then(|slot| slot.pending.as_ref())
            .filter(|(_, published)| *published)
            .map(|(p, _)| p.clone())
    }

    pub(crate) fn consume(&self, proposal: &Proposal) {
        let mut slots = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(slot) = slots.get_mut(&proposal.conversation_id) {
            if slot
                .lookup
                .as_ref()
                .is_some_and(|p| p.lookup_id == proposal.lookup_id)
            {
                slot.lookup = None;
            }
            if slot
                .pending
                .as_ref()
                .is_some_and(|(p, _)| p.lookup_id == proposal.lookup_id)
            {
                slot.pending = None;
            }
        }
    }

    /// Only the proposal owner can publish or withdraw it; older concurrent turns cannot erase it.
    pub(crate) fn finish_turn(&self, conversation: &str, run: &str, published: bool) {
        let mut slots = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(slot) = slots.get_mut(conversation) {
            if slot.lookup.as_ref().is_some_and(|p| p.run_id == run) {
                slot.lookup = None;
            }
            if let Some((p, active)) = &mut slot.pending {
                if p.run_id == run {
                    if published {
                        *active = true;
                    } else {
                        slot.pending = None;
                    }
                }
            }
            if slot.pending.is_none() && slot.lookup.is_none() {
                slots.remove(conversation);
            }
        }
    }

    pub(crate) fn cancel_run(&self, run: &str) {
        let mut slots = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        for slot in slots.values_mut() {
            if slot.lookup.as_ref().is_some_and(|p| p.run_id == run) {
                slot.lookup = None;
            }
            if slot.pending.as_ref().is_some_and(|(p, _)| p.run_id == run) {
                slot.pending = None;
            }
        }
        slots.retain(|_, slot| slot.lookup.is_some() || slot.pending.is_some());
    }
}
