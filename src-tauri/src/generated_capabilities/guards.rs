//! Guards around verification and activation: input validation, the terminal recording of a
//! check, and the promotion precondition that ties a passed check to the live runtime and the
//! intact managed payload.

use serde_json::{Map, Value};

use super::{
    contracts::WasmContract,
    errors::*,
    lifecycle,
    repository::{self, Activation, RevisionState},
    verification::VerificationHashes,
};
use crate::{now_iso, persistence::SqliteWriter};

/// The terminal result of one verification check.
pub struct CheckOutcome {
    pub check_id: String,
    pub passed: bool,
    pub error_code: Option<CapabilityErrorCode>,
    pub runtime_digest: String,
    pub hashes: Option<VerificationHashes>,
    pub report_ref: String,
}

/// Records the terminal status of a check and, only when the revision is still verifiable,
/// promotes it to `validated`. The two are deliberately separate: a revision stopped or
/// replaced while verification ran keeps its state, while the check must still reach a terminal
/// status so that it never stays `running`.
pub(crate) fn finalize_check(
    writer: &SqliteWriter,
    revision_id: &str,
    outcome: &CheckOutcome,
) -> CapabilityResult<bool> {
    lifecycle::transaction(writer, |transaction| {
        let current = repository::revision_by_id(transaction, revision_id)?;
        let verifiable = lifecycle::state_is_verifiable(current.state);
        let validated = outcome.passed && verifiable;
        if validated {
            if let Some(hashes) = &outcome.hashes {
                repository::record_hashes(
                    transaction,
                    revision_id,
                    &hashes.source_hash,
                    &hashes.program_hash,
                    &hashes.artifact_hash,
                )?;
            }
            repository::update_runtime_digest(transaction, revision_id, &outcome.runtime_digest)?;
            repository::set_revision_state(transaction, revision_id, RevisionState::Validated)?;
            repository::finish_check(
                transaction,
                &outcome.check_id,
                "passed",
                None,
                Some(&outcome.report_ref),
                &now_iso(),
            )?;
        } else {
            let code = if verifiable {
                outcome.error_code
            } else {
                Some(CapabilityErrorCode::NotValidated)
            };
            repository::finish_check(
                transaction,
                &outcome.check_id,
                "failed",
                code.as_ref().map(|code| code.as_str()),
                Some(&outcome.report_ref),
                &now_iso(),
            )?;
        }
        Ok(validated)
    })
}

/// Inputs must carry exactly the contract's fields, all as JSON booleans, before any spawn.
pub fn validate_input(contract: &WasmContract, input: &Map<String, Value>) -> CapabilityResult<()> {
    if input.len() != contract.fields.len()
        || contract
            .fields
            .iter()
            .any(|field| !matches!(input.get(&field.name), Some(Value::Bool(_))))
    {
        return error(
            CapabilityErrorCode::InvalidInput,
            "input must contain exactly the contract's boolean fields",
        );
    }
    Ok(())
}

/// Promotes a validated revision at the expected catalog epoch. The caller supplies the live
/// runtime digest and managed-copy inventory digest; both are compared with the stored revision
/// inside the transaction, so a promotion cannot rest on a check made against a different
/// runtime or on a payload changed after verification.
pub(crate) fn activate_revision(
    writer: &SqliteWriter,
    revision_id: &str,
    expected_epoch: i64,
    runtime_digest: &str,
    inventory_hash: &str,
) -> CapabilityResult<Activation> {
    lifecycle::transaction(writer, |transaction| {
        let revision = repository::revision_by_id(transaction, revision_id)?;
        if revision.runtime_digest != runtime_digest {
            return error(
                CapabilityErrorCode::NotValidated,
                "revision must be re-verified for the current runtime",
            );
        }
        if revision.inventory_hash != inventory_hash {
            return error(
                CapabilityErrorCode::IntegrityError,
                "managed package contents changed since verification",
            );
        }
        repository::activate(transaction, revision_id, expected_epoch, &now_iso())
    })
}
