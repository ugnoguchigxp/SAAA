//! Explicit approval gate for premium reasoning proposals.
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

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
    connection.execute("UPDATE rr_premium_proposals SET status='approved',approved_at_ms=?1 WHERE id=?2 AND status='proposed'", params![now_ms, proposal.id]).map_err(|error| error.to_string())?;
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
    }
}
