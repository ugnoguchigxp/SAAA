//! Authority records are distinct from model-proposed text.
use super::contracts::{GoalProposal, Operation};
use crate::{database_error, now_iso};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SourceBinding {
    pub kind: SourceKind,
    pub id: String,
    pub version: String,
    pub quote_start: Option<u32>,
    pub quote_end: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum SourceKind {
    UserMessage,
    UiReceipt,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ConfirmationReceipt {
    pub proposal_id: String,
    pub expected_revision: i64,
    pub display_digest: String,
}

#[derive(Debug, Clone)]
pub(crate) struct ExistingGrant {
    pub goal_id: String,
    pub delegation_id: String,
    pub workspace_id: String,
    pub ops: String,
    pub budget_runs: i64,
    pub budget_ms: i64,
    pub expires_at: Option<String>,
    pub target: Option<String>,
    pub recipe_id: Option<String>,
    pub revision: i64,
}

pub(crate) fn unicode_slice(text: &str, start: u32, end: u32) -> Result<String, &'static str> {
    if end < start {
        return Err("source_quote_invalid");
    }
    let chars: Vec<char> = text.chars().collect();
    let start = start as usize;
    let end = end as usize;
    if end > chars.len() {
        return Err("source_quote_invalid");
    }
    Ok(chars[start..end].iter().collect())
}

pub(crate) fn binding_from_message(
    connection: &Connection,
    conversation_id: &str,
    message_id: &str,
    quote_start: Option<u32>,
    quote_end: Option<u32>,
) -> Result<(SourceBinding, String), String> {
    let (role, content): (String, String) = connection
        .query_row(
            "SELECT role,content FROM conversation_messages WHERE id=?1 AND conversation_id=?2",
            params![message_id, conversation_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(database_error)?
        .ok_or("source_unavailable")?;
    if !matches!(role.as_str(), "user" | "transcript") {
        return Err("source_unavailable".into());
    }
    let version = source_digest(&content);
    if let (Some(start), Some(end)) = (quote_start, quote_end) {
        let quoted = unicode_slice(&content, start, end)?;
        if quoted.is_empty() {
            return Err("source_quote_invalid".into());
        }
        return Ok((
            SourceBinding {
                kind: SourceKind::UserMessage,
                id: message_id.into(),
                version,
                quote_start,
                quote_end,
            },
            quoted,
        ));
    }
    Ok((
        SourceBinding {
            kind: SourceKind::UserMessage,
            id: message_id.into(),
            version,
            quote_start: None,
            quote_end: None,
        },
        content,
    ))
}

pub(crate) fn source_digest(text: &str) -> String {
    format!("{:x}", Sha256::digest(text.as_bytes()))
}

pub(crate) fn current_source_digest(
    connection: &Connection,
    conversation_id: &str,
    message_id: &str,
) -> Result<String, String> {
    let (role, content): (String, String) = connection
        .query_row(
            "SELECT role,content FROM conversation_messages WHERE id=?1 AND conversation_id=?2",
            params![message_id, conversation_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(database_error)?
        .ok_or("source_unavailable")?;
    if !matches!(role.as_str(), "user" | "transcript") {
        return Err("source_unavailable".into());
    }
    Ok(source_digest(&content))
}

pub(crate) fn covering_grant(
    connection: &Connection,
    conversation_id: &str,
    proposal: &GoalProposal,
) -> Result<Option<ExistingGrant>, String> {
    let ops = proposal.operations_key();
    let mut statement = connection
        .prepare(
            "SELECT g.id,d.id,d.workspace_id,d.ops,d.budget_runs,d.budget_ms,d.expires_at,d.target,d.recipe_id,d.revision
             FROM steward_goals g JOIN steward_delegations d ON d.goal_id=g.id
             JOIN steward_goal_progress p ON p.goal_id=g.id
             WHERE g.conversation_id=?1 AND g.status='active' AND g.superseded_by IS NULL
               AND d.status='active' AND d.superseded_by IS NULL
               AND p.work_status NOT IN ('done','failed','cancelled')
               AND d.workspace_id=?2
             ORDER BY g.rowid",
        )
        .map_err(database_error)?;
    let rows = statement
        .query_map(params![conversation_id, proposal.workspace_id], |row| {
            Ok(ExistingGrant {
                goal_id: row.get(0)?,
                delegation_id: row.get(1)?,
                workspace_id: row.get(2)?,
                ops: row.get(3)?,
                budget_runs: row.get(4)?,
                budget_ms: row.get(5)?,
                expires_at: row.get(6)?,
                target: row.get(7)?,
                recipe_id: row.get(8)?,
                revision: row.get(9)?,
            })
        })
        .map_err(database_error)?;
    for row in rows {
        let grant = row.map_err(database_error)?;
        if !ops_contains(&grant.ops, ops) {
            continue;
        }
        if let Some(expiry) = &grant.expires_at {
            if expiry.as_str() < now_iso().as_str() {
                continue;
            }
        }
        if let Some(target) = &proposal.target {
            if grant.target.as_deref() != Some(target.as_str()) {
                continue;
            }
        }
        if let Some(recipe) = &proposal.recipe_id {
            if grant.recipe_id.as_deref() != Some(recipe.as_str()) {
                continue;
            }
        }
        let reserved: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM steward_budget_reservations WHERE delegation_id=?1 AND state IN ('reserved','consumed')",
                [&grant.delegation_id],
                |row| row.get(0),
            )
            .map_err(database_error)?;
        if reserved >= grant.budget_runs {
            continue;
        }
        if proposal.budget_ms as i64 > grant.budget_ms {
            continue;
        }
        return Ok(Some(grant));
    }
    Ok(None)
}

fn ops_contains(grant: &str, requested: &str) -> bool {
    match (grant, requested) {
        ("read_test", _) => true,
        (same, req) if same == req => true,
        ("read", "read") | ("test_run", "test_run") => true,
        _ => false,
    }
}

pub(crate) fn operations_cover(operations: &[Operation], requested: &[Operation]) -> bool {
    requested.iter().all(|item| {
        operations
            .iter()
            .any(|granted| std::mem::discriminant(granted) == std::mem::discriminant(item))
    })
}

pub(crate) fn persist_binding(
    connection: &Connection,
    subject_kind: &str,
    subject_id: &str,
    binding: &SourceBinding,
    scope_key: Option<&str>,
    scope_epoch: Option<i64>,
    goal_revision: Option<i64>,
    delegation_revision: Option<i64>,
) -> Result<(), String> {
    connection
        .execute(
            "INSERT INTO steward_source_bindings(
               id,subject_kind,subject_id,source_kind,source_id,source_version,quote_start,quote_end,
               scope_key,scope_epoch,goal_revision,delegation_revision,created_at)
             VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13)",
            params![
                crate::new_id("bind"),
                subject_kind,
                subject_id,
                match binding.kind {
                    SourceKind::UserMessage => "user_message",
                    SourceKind::UiReceipt => "ui_receipt",
                },
                binding.id,
                binding.version,
                binding.quote_start.map(i64::from),
                binding.quote_end.map(i64::from),
                scope_key,
                scope_epoch,
                goal_revision,
                delegation_revision,
                now_iso(),
            ],
        )
        .map_err(database_error)?;
    Ok(())
}
