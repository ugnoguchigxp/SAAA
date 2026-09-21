//! Database revalidation at result acceptance.
use super::*;
impl WorldFrameService {
    /// Rechecks database-owned dependencies inside the generation acceptance transaction.
    /// Situation is checked outside it; its mutex is never nested under the writer lock.
    pub(crate) fn validate_db_result(
        &self,
        c: &Connection,
        prior: &PreparedWorldFrame,
    ) -> Result<(), FrameError> {
        let request = prior.request.as_borrowed(prior.request.access.as_request());
        let auth = authorize_frame_request(c, &request)?;
        if auth.scope_digest != prior.stamp.scope_digest
            || auth.ledger_revision != prior.stamp.ledger_revision
            || auth.input_epoch != prior.stamp.input_epoch
            || auth.policy_revision != prior.stamp.policy_revision
        {
            return Err(FrameError::Changed);
        }
        let mut current = prior.frame.clone();
        if self.sources_enabled() {
            let situation = current.sources.iter().find(|s| s.kind == saaa_personal_state_core::world::frame_sources::WorldSourceKind::Situation).cloned();
            current.sources = super::super::runtime_sources::read(c, &auth, (self.clock)())?;
            current.sources.extend(situation);
        }
        for target in &auth.targets {
            let view = runtime_coding::read_view(c, &auth, &target.reference)?;
            let prior_view = current
                .runtime
                .iter()
                .find(|v| v.reference == target.reference);
            if prior_view.map(|v| &v.owner_digest)
                != view.unit.as_ref().map(|v| &v.view.owner_digest)
            {
                return Err(FrameError::Changed);
            }
        }
        if content_digest(&current)? != prior.stamp.content_digest {
            return Err(FrameError::Changed);
        }
        Ok(())
    }
}
