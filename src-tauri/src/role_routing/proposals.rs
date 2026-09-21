//! Explicit approval gate for premium reasoning proposals.
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Proposal {
    pub(crate) id: String,
    pub(crate) root_id: String,
    pub(crate) candidate_id: String,
    pub(crate) policy_id: String,
    pub(crate) revision: u32,
    pub(crate) expires_at_ms: i64,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct Approval {
    pub(crate) proposal_id: String,
    /// The UI must name the offered candidate. A generic affirmative is intentionally not an
    /// approval because it cannot be bound to a price or actor.
    pub(crate) candidate_id: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ProposalReceipt {
    pub(crate) id: String,
    pub(crate) root_id: String,
    pub(crate) candidate_id: String,
    pub(crate) expires_at_ms: i64,
    pub(crate) status: String,
}

pub(crate) fn record(
    connection: &Connection,
    proposal: &Proposal,
    estimated_cost_micros: Option<u64>,
    now_ms: i64,
) -> Result<ProposalReceipt, String> {
    if proposal.id.is_empty() || proposal.root_id.is_empty() || proposal.candidate_id.is_empty() {
        return Err("Role-routing premium proposal is incomplete".into());
    }
    if proposal.expires_at_ms <= now_ms {
        return Err("Role-routing premium proposal is already expired".into());
    }
    let (policy_id, revision, phase): (String, u32, String) = connection
        .query_row(
            "SELECT policy_id,revision,phase FROM rr_roots WHERE root_id=?1",
            [&proposal.root_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .map_err(|_| "Role-routing premium proposal root is unavailable".to_string())?;
    if phase != "responding" || policy_id != proposal.policy_id || revision != proposal.revision {
        return Err("Role-routing premium proposal is stale".into());
    }
    let settings_json: String = connection
        .query_row(
            "SELECT config_json FROM rr_policy_versions WHERE id=?1",
            [&policy_id],
            |row| row.get(0),
        )
        .map_err(|_| "Role-routing premium proposal policy is unavailable".to_string())?;
    let settings: crate::role_routing::RoleRoutingSettings =
        serde_json::from_str(&settings_json)
            .map_err(|_| "Role-routing premium proposal policy is invalid".to_string())?;
    if settings.premium_approval != "per_request"
        || settings.roles.premium.as_deref() != Some(proposal.candidate_id.as_str())
        || !settings
            .actors
            .iter()
            .any(|actor| actor.id == proposal.candidate_id && actor.location == "cloud")
    {
        return Err("Role-routing premium proposal candidate is not eligible".into());
    }
    if let Some(limit) = settings.limits.max_estimated_cost_micros {
        if estimated_cost_micros.is_none_or(|cost| cost > limit) {
            return Err(
                "Role-routing premium proposal cost is unavailable or exceeds budget".into(),
            );
        }
    }
    connection.execute(
        "INSERT INTO rr_premium_proposals(id,root_id,candidate_id,policy_id,revision,estimated_cost_micros,expires_at_ms,status,created_at_ms) VALUES(?1,?2,?3,?4,?5,?6,?7,'proposed',?8)",
        params![proposal.id, proposal.root_id, proposal.candidate_id, proposal.policy_id, proposal.revision, estimated_cost_micros, proposal.expires_at_ms, now_ms],
    ).map_err(|error| error.to_string())?;
    Ok(ProposalReceipt {
        id: proposal.id.clone(),
        root_id: proposal.root_id.clone(),
        candidate_id: proposal.candidate_id.clone(),
        expires_at_ms: proposal.expires_at_ms,
        status: "proposed".into(),
    })
}

/// Rechecks the current root binding immediately before accepting approval.  It records consent
/// only; a dispatch owner must still perform its own provider/capability check before execution.
pub(crate) fn approve(
    connection: &Connection,
    approval: &Approval,
    now_ms: i64,
    cloud_allowed: bool,
) -> Result<ProposalReceipt, String> {
    let proposal: Proposal = connection.query_row(
        "SELECT id,root_id,candidate_id,policy_id,revision,expires_at_ms FROM rr_premium_proposals WHERE id=?1 AND status='proposed'",
        [&approval.proposal_id],
        |row| Ok(Proposal { id: row.get(0)?, root_id: row.get(1)?, candidate_id: row.get(2)?, policy_id: row.get(3)?, revision: row.get(4)?, expires_at_ms: row.get(5)? }),
    ).optional().map_err(|error| error.to_string())?.ok_or_else(|| "Role-routing premium proposal is unavailable".to_string())?;
    let (policy_id, revision): (String, u32) = connection
        .query_row(
            "SELECT policy_id,revision FROM rr_roots WHERE root_id=?1 AND phase='responding'",
            [&proposal.root_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(|_| "Role-routing premium proposal is stale".to_string())?;
    if !may_dispatch(
        &proposal,
        Some(&approval.candidate_id),
        &policy_id,
        revision,
        now_ms,
        cloud_allowed,
    ) {
        return Err("Role-routing premium approval failed revalidation".into());
    }
    let changed = connection.execute("UPDATE rr_premium_proposals SET status='approved',approved_at_ms=?1 WHERE id=?2 AND status='proposed'", params![now_ms, proposal.id]).map_err(|error| error.to_string())?;
    if changed != 1 {
        return Err("Role-routing premium proposal changed during approval".into());
    }
    Ok(ProposalReceipt {
        id: proposal.id,
        root_id: proposal.root_id,
        candidate_id: proposal.candidate_id,
        expires_at_ms: proposal.expires_at_ms,
        status: "approved".into(),
    })
}

pub(crate) fn may_dispatch(
    proposal: &Proposal,
    approved_candidate: Option<&str>,
    policy_id: &str,
    revision: u32,
    now_ms: i64,
    cloud_allowed: bool,
) -> bool {
    cloud_allowed
        && now_ms <= proposal.expires_at_ms
        && proposal.policy_id == policy_id
        && proposal.revision == revision
        && approved_candidate == Some(proposal.candidate_id.as_str())
}

/// Rechecks mutable host capability for the exact cloud actor named by a persisted proposal.
/// Policy identity is immutable, but provider enablement, Codex health, model, and location are
/// deliberately live facts and may be revoked after the proposal was created.
pub(crate) fn candidate_available(
    connection: &Connection,
    policy_id: &str,
    candidate_id: &str,
) -> Result<bool, String> {
    let settings_json: String = connection
        .query_row(
            "SELECT config_json FROM rr_policy_versions WHERE id=?1",
            [policy_id],
            |row| row.get(0),
        )
        .map_err(|_| "Role-routing premium proposal policy is unavailable".to_string())?;
    let settings: crate::role_routing::RoleRoutingSettings =
        serde_json::from_str(&settings_json)
            .map_err(|_| "Role-routing premium proposal policy is invalid".to_string())?;
    let Some(actor) = settings
        .actors
        .iter()
        .find(|actor| actor.id == candidate_id)
    else {
        return Ok(false);
    };
    if actor.location != "cloud" {
        return Ok(false);
    }
    match actor.transport.as_str() {
        "codex_sdk" => {
            let codex = crate::persistence::load_codex_settings(connection)?;
            Ok(codex.enabled
                && codex.health == "ready"
                && actor.model.as_deref() == Some(codex.model.as_str()))
        }
        "provider" => {
            let Some(provider_id) = actor.provider_id.as_deref() else {
                return Ok(false);
            };
            let providers = crate::persistence::load_model_providers(connection)?;
            Ok(providers.providers.iter().any(|provider| {
                provider.id() == provider_id
                    && provider.enabled()
                    && provider.location() == actor.location
                    && matches!(
                        provider,
                        crate::ModelProviderSettings::OpenAiCompatible(_)
                            | crate::ModelProviderSettings::AgentSession(_)
                            | crate::ModelProviderSettings::DynamicLan(_)
                    )
            }))
        }
        _ => Ok(false),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ConsumedApproval {
    pub(crate) proposal_id: String,
    pub(crate) root_id: String,
    pub(crate) actor_id: String,
    pub(crate) step_id: String,
}

/// Declines a proposal. A declined proposal can never be approved afterwards.
pub(crate) fn decline(
    connection: &Connection,
    proposal_id: &str,
    now_ms: i64,
) -> Result<(), String> {
    let changed = connection
        .execute(
            "UPDATE rr_premium_proposals SET status='declined' WHERE id=?1 AND status='proposed'",
            params![proposal_id],
        )
        .map_err(|error| error.to_string())?;
    let _ = now_ms;
    if changed != 1 {
        return Err("Role-routing premium proposal is not declinable".into());
    }
    Ok(())
}

/// Consumes an approved proposal exactly once and creates the premium step in the same ambient
/// transaction, so a crash cannot leave a consumed approval without its step (or vice versa).
/// The current policy, revision, expiry, cloud permission, and named candidate are rechecked at
/// this dispatch boundary.
#[allow(clippy::too_many_arguments)]
pub(crate) fn consume_approval(
    connection: &Connection,
    approval: &Approval,
    expected_policy_id: &str,
    expected_revision: u32,
    cloud_allowed: bool,
    now_ms: i64,
) -> Result<ConsumedApproval, String> {
    let proposal: Proposal = connection
        .query_row(
            "SELECT id,root_id,candidate_id,policy_id,revision,expires_at_ms FROM rr_premium_proposals WHERE id=?1 AND status='approved' AND consumed_at_ms IS NULL",
            [&approval.proposal_id],
            |row| {
                Ok(Proposal {
                    id: row.get(0)?,
                    root_id: row.get(1)?,
                    candidate_id: row.get(2)?,
                    policy_id: row.get(3)?,
                    revision: row.get(4)?,
                    expires_at_ms: row.get(5)?,
                })
            },
        )
        .optional()
        .map_err(|error| error.to_string())?
        .ok_or_else(|| "Role-routing premium approval is unavailable or already consumed".to_string())?;
    if now_ms > proposal.expires_at_ms {
        let changed = connection
            .execute(
                "UPDATE rr_premium_proposals SET status='expired' WHERE id=?1 AND status='approved' AND consumed_at_ms IS NULL",
                [&proposal.id],
            )
            .map_err(|error| error.to_string())?;
        if changed != 1 {
            return Err("Role-routing premium approval changed during expiry".into());
        }
        return Err("Role-routing premium approval has expired".into());
    }
    if proposal.policy_id != expected_policy_id
        || proposal.revision != expected_revision
        || !may_dispatch(
            &proposal,
            Some(&approval.candidate_id),
            expected_policy_id,
            expected_revision,
            now_ms,
            cloud_allowed,
        )
    {
        return Err("Role-routing premium approval failed revalidation".into());
    }
    let settings_json: String = connection
        .query_row(
            "SELECT config_json FROM rr_policy_versions WHERE id=?1",
            [&proposal.policy_id],
            |row| row.get(0),
        )
        .map_err(|_| "Role-routing premium approval policy is unavailable".to_string())?;
    let settings: crate::role_routing::RoleRoutingSettings =
        serde_json::from_str(&settings_json)
            .map_err(|_| "Role-routing premium approval policy is invalid".to_string())?;
    if settings.roles.premium.as_deref() != Some(proposal.candidate_id.as_str()) {
        return Err("Role-routing premium candidate changed; propose again".into());
    }
    let ordinal: i64 = connection
        .query_row(
            "SELECT COALESCE(MAX(ordinal),-1)+1 FROM rr_steps WHERE root_id=?1",
            [&proposal.root_id],
            |row| row.get(0),
        )
        .map_err(|error| error.to_string())?;
    let step_id = format!("rr-step-{}-{}", proposal.root_id, ordinal);
    let fingerprint = format!(
        "{:x}",
        Sha256::digest(
            format!("{}:{}:premium", proposal.policy_id, proposal.candidate_id).as_bytes()
        )
    );
    connection
        .execute(
            "INSERT INTO rr_steps(id,root_id,revision,ordinal,actor_id,purpose,status,config_fingerprint,adapter_state_json) VALUES(?1,?2,?3,?4,?5,'reconsider','planned',?6,'{}')",
            params![step_id, proposal.root_id, i64::from(proposal.revision), ordinal, proposal.candidate_id, fingerprint],
        )
        .map_err(|error| error.to_string())?;
    let changed = connection
        .execute(
            "UPDATE rr_premium_proposals SET consumed_at_ms=?1 WHERE id=?2 AND status='approved' AND consumed_at_ms IS NULL",
            params![now_ms, proposal.id],
        )
        .map_err(|error| error.to_string())?;
    if changed != 1 {
        return Err("Role-routing premium approval was consumed concurrently".into());
    }
    Ok(ConsumedApproval {
        proposal_id: proposal.id,
        root_id: proposal.root_id,
        actor_id: proposal.candidate_id,
        step_id,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn proposal() -> Proposal {
        Proposal {
            id: "proposal-1".into(),
            root_id: "root-1".into(),
            candidate_id: "astra".into(),
            policy_id: "policy-1".into(),
            revision: 2,
            expires_at_ms: 100,
        }
    }

    #[test]
    fn rr_26_premium_no_implicit_execution() {
        assert!(!may_dispatch(&proposal(), None, "policy-1", 2, 10, true));
        assert!(may_dispatch(
            &proposal(),
            Some("astra"),
            "policy-1",
            2,
            10,
            true
        ));
    }

    #[test]
    fn rr_26_stale_or_cloud_forbidden_is_rejected() {
        assert!(!may_dispatch(
            &proposal(),
            Some("astra"),
            "policy-1",
            3,
            10,
            true
        ));
        assert!(!may_dispatch(
            &proposal(),
            Some("astra"),
            "policy-1",
            2,
            101,
            true
        ));
        assert!(!may_dispatch(
            &proposal(),
            Some("astra"),
            "policy-1",
            2,
            10,
            false
        ));
    }

    #[test]
    fn rr_26_receipt_requires_named_candidate_and_rechecks_revision() {
        let connection = Connection::open_in_memory().expect("connection");
        connection.execute_batch("PRAGMA foreign_keys=ON; CREATE TABLE conversations(id TEXT PRIMARY KEY); CREATE TABLE runtime_runs(id TEXT PRIMARY KEY); CREATE TABLE conversation_messages(id TEXT PRIMARY KEY); INSERT INTO conversations VALUES('c');").expect("base");
        crate::role_routing::schema::migrate(&connection).expect("schema");
        let settings = serde_json::json!({"schemaVersion":1,"enabled":true,"actors":[{"id":"astra","label":"Astra","aliases":[],"transport":"codex_sdk","providerId":null,"model":"gpt-6-astra","location":"cloud","resourceGroup":"cloud","maxInputBytes":1024,"capabilities":[]}],"roles":{"frontend":null,"reasoner":"astra","advanced":null,"reviewer":null,"premium":"astra","toolSpecialist":null},"recipes":[{"id":"respond","action":"respond","roles":["reasoner"],"enabled":true}],"limits":{"maxReasoningSteps":4,"maxToolCalls":32,"rootTimeoutMs":180000,"stepTimeoutMs":60000,"frontendTimeoutMs":1200,"classificationTimeoutMs":1500,"maxQueuedInputs":4,"maxReviewRounds":1,"maxAutomaticSwitches":2,"maxEstimatedCostMicros":100},"speech":{"mode":"author_verbatim","ackDelayMs":250,"maxAckChars":80,"progressMinIntervalMs":15000,"maxProgressPerRoot":2},"selection":{"mode":"rules","shadowArtifactId":null,"classificationMinConfidence":0.85,"weights":{"quality":0.6,"latency":0.25,"cost":0.15},"switchMargin":0.15},"premiumApproval":"per_request","learning":{"enabled":false,"localStart":"02:00","localEnd":"05:00","idleSeconds":300,"maxRunSeconds":600,"batchSize":100,"allowLocalLabeler":false},"adaptiveImprovement":{"enabled":false,"providerRecipe":false,"tool":false,"plan":false,"notification":false}}).to_string();
        connection
            .execute(
                "INSERT INTO rr_policy_versions VALUES('p',1,?1,'d',1)",
                [&settings],
            )
            .expect("policy");
        connection.execute("INSERT INTO rr_roots(root_id,conversation_id,policy_id,revision,phase,origin,presentation_mode,started_at_ms,scope_digest) VALUES('root-1','c','p',2,'responding','text','visual',1,'')", []).expect("root");
        let mut proposal = proposal();
        proposal.policy_id = "p".into();
        record(&connection, &proposal, Some(10), 10).expect("proposal");
        assert!(approve(
            &connection,
            &Approval {
                proposal_id: "proposal-1".into(),
                candidate_id: "yes".into()
            },
            11,
            true
        )
        .is_err());
        assert_eq!(
            approve(
                &connection,
                &Approval {
                    proposal_id: "proposal-1".into(),
                    candidate_id: "astra".into()
                },
                11,
                true
            )
            .expect("approval")
            .status,
            "approved"
        );
        let consumed = consume_approval(
            &connection,
            &Approval {
                proposal_id: "proposal-1".into(),
                candidate_id: "astra".into(),
            },
            "p",
            2,
            true,
            12,
        )
        .expect("consume");
        assert_eq!(consumed.actor_id, "astra");
        assert_eq!(
            connection
                .query_row(
                    "SELECT actor_id FROM rr_steps WHERE id=?1",
                    [&consumed.step_id],
                    |row| row.get::<_, String>(0)
                )
                .expect("step"),
            "astra"
        );
        // A second consumption of the same approval must not create a second step.
        assert!(consume_approval(
            &connection,
            &Approval {
                proposal_id: "proposal-1".into(),
                candidate_id: "astra".into(),
            },
            "p",
            2,
            true,
            13,
        )
        .is_err());
        assert_eq!(
            connection
                .query_row(
                    "SELECT count(*) FROM rr_steps WHERE purpose='reconsider'",
                    [],
                    |row| row.get::<_, i64>(0)
                )
                .expect("count"),
            1
        );
    }
}
