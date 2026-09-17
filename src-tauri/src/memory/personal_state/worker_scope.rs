use super::{jobs::Job, store};
use rusqlite::Connection;
use saaa_personal_state_core::{CommitContext, StatePatch};

pub(super) fn request(job: &Job, source_id: &str) -> Option<String> {
    match job.scope_key.as_deref() {
        Some(scope) if scope.starts_with("user:") => None,
        Some(scope) => Some(scope.to_string()),
        None => Some(source_id.to_string()),
    }
}

pub(super) fn rebase(
    connection: &Connection,
    job: &Job,
    patch: &mut StatePatch,
    context: &mut CommitContext<'_>,
) -> Result<(), String> {
    if job.scope_key.is_none() {
        return Ok(());
    }
    let current = store::load(connection)?;
    if current.policy_revision != patch.policy_revision {
        return Err("personal-job-policy-fence".into());
    }
    patch.base_revision = current.revision;
    patch.input_epoch = current.input_epoch;
    let mut sequence = current
        .transitions
        .last()
        .map_or(1, |value| value.sequence + 1);
    for transition in &mut patch.transitions {
        transition.sequence = sequence;
        sequence += 1;
    }
    context.access.policy_revision = current.policy_revision;
    Ok(())
}
