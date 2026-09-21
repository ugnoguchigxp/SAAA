//! Executor control plane for role routing.
//!
//! This module is the single place that answers "may the host dispatch the next step, and with
//! which actor?". It combines the compiled recipe, the root budget, the root/step deadlines, and
//! the active step claim so the runtime cannot bypass an invariant by calling a lower-level helper.
//! It performs no provider, tool, or speech I/O, and never holds a database transaction across an
//! await point.
#![allow(dead_code)]

use super::contracts::RoleRoutingSettings;
use super::limits::BudgetState;
use super::recipe::{CompiledRecipe, PlannedStep};
use rusqlite::{Connection, OptionalExtension};

/// The host is allowed to dispatch exactly this step.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DispatchPermit {
    pub(crate) root_id: String,
    pub(crate) revision: u32,
    pub(crate) step_id: String,
    pub(crate) ordinal: u32,
    pub(crate) actor_id: String,
    pub(crate) purpose: String,
    pub(crate) config_fingerprint: String,
}

/// Loads the cumulative root budget from the ledger. Steps count executed reasoning steps, tool
/// links count external tool operations, and `root_resumed` events count automatic switches.
pub(crate) fn load_budget(connection: &Connection, root_id: &str) -> Result<BudgetState, String> {
    let steps: i64 = connection
        .query_row(
            "SELECT count(*) FROM rr_steps WHERE root_id=?1 AND (status IN ('running','draining','succeeded','failed','interrupted') OR (status='cancelled' AND started_at_ms IS NOT NULL))",
            [root_id],
            |row| row.get(0),
        )
        .map_err(|error| error.to_string())?;
    let tool_calls: i64 = connection
        .query_row(
            "SELECT count(*) FROM rr_tool_links WHERE root_id=?1",
            [root_id],
            |row| row.get(0),
        )
        .map_err(|error| error.to_string())?;
    let automatic_switches: i64 = connection
        .query_row(
            "SELECT count(*) FROM rr_events WHERE root_id=?1 AND kind='root_resumed'",
            [root_id],
            |row| row.get(0),
        )
        .map_err(|error| error.to_string())?;
    let review_rounds: i64 = connection
        .query_row(
            "SELECT count(*) FROM rr_steps WHERE root_id=?1 AND purpose='review' AND started_at_ms IS NOT NULL",
            [root_id],
            |row| row.get(0),
        )
        .map_err(|error| error.to_string())?;
    let spent_cost_micros: i64 = connection
        .query_row(
            "SELECT COALESCE(SUM(COALESCE(json_extract(usage_json,'$.estimatedCostMicros'),0)),0) FROM rr_steps WHERE root_id=?1",
            [root_id],
            |row| row.get(0),
        )
        .map_err(|error| error.to_string())?;
    Ok(BudgetState {
        steps: u8::try_from(steps).unwrap_or(u8::MAX),
        tool_calls: u8::try_from(tool_calls).unwrap_or(u8::MAX),
        automatic_switches: u8::try_from(automatic_switches).unwrap_or(u8::MAX),
        review_rounds: u8::try_from(review_rounds).unwrap_or(u8::MAX),
        spent_cost_micros: u64::try_from(spent_cost_micros).unwrap_or(0),
    })
}

/// Compiles a specific recipe through the executor boundary so callers do not import the raw
/// compiler directly.
pub(crate) fn compile(
    settings: &RoleRoutingSettings,
    recipe_id: &str,
) -> Result<CompiledRecipe, String> {
    super::recipe::compile_recipe_by_id(settings, recipe_id)
}

/// Projects a step's Must context. Re-exported here so the runtime only talks to the executor.
pub(crate) fn project_context(
    root_input: &super::context::ContextCondition,
    amendments: &[super::context::ContextCondition],
    allowed_scopes: &[String],
    generation: u32,
    require_non_empty: bool,
) -> Result<super::context::RoleContext, super::context::ContextViolation> {
    super::context::project_role_context(
        root_input,
        amendments,
        allowed_scopes,
        generation,
        require_non_empty,
    )
}

/// Checks that the already-claimed active step still fits the cumulative budget. Running steps
/// are included in `load_budget`, so this check must not reserve the current step a second time.
pub(crate) fn ensure_step_budget(
    connection: &Connection,
    settings: &RoleRoutingSettings,
    root_id: &str,
) -> Result<(), String> {
    let budget = load_budget(connection, root_id)?;
    if budget.steps > settings.limits.max_reasoning_steps {
        return Err("Role-routing step budget exceeded".into());
    }
    if let Some(max_cost) = settings.limits.max_estimated_cost_micros {
        if budget.spent_cost_micros > max_cost {
            return Err("Role-routing cost budget exceeded".into());
        }
    }
    Ok(())
}

/// Checks whether the root's next reasoning step may start. Returns `Ok(None)` when the root is
/// intentionally queued (another root owns the conversation), an error when the root is terminal
/// or the budget/deadline is exhausted, and a permit when dispatch is allowed.
pub(crate) fn permit_next_step(
    connection: &Connection,
    settings: &RoleRoutingSettings,
    root_id: &str,
    now_ms: i64,
) -> Result<Option<DispatchPermit>, String> {
    let root: Option<(String, i64, Option<i64>)> = connection
        .query_row(
            "SELECT phase,revision,deadline_at_ms FROM rr_roots WHERE root_id=?1",
            [root_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()
        .map_err(|error| error.to_string())?;
    let Some((phase, revision, deadline_at_ms)) = root else {
        return Err("Role-routing root was not found".into());
    };
    match phase.as_str() {
        "queued" => return Ok(None),
        "cancelled" | "failed" | "completed" => {
            return Err(format!("Role-routing root is terminal ({phase})"))
        }
        "draining" => return Err("Role-routing root is draining".into()),
        "responding" => {}
        _ => return Err("Role-routing root has an invalid phase".into()),
    }
    if deadline_at_ms.is_some_and(|deadline| now_ms >= deadline) {
        return Err("Role-routing root deadline reached".into());
    }
    let step: Option<(String, i64, u32, String, String, String, Option<i64>)> = connection
        .query_row(
            "SELECT id,revision,ordinal,actor_id,purpose,config_fingerprint,started_at_ms FROM rr_steps WHERE root_id=?1 AND status='running' ORDER BY ordinal LIMIT 1",
            [root_id],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                    row.get(6)?,
                ))
            },
        )
        .optional()
        .map_err(|error| error.to_string())?;
    let Some((
        step_id,
        step_revision,
        ordinal,
        actor_id,
        purpose,
        config_fingerprint,
        started_at_ms,
    )) = step
    else {
        return Err("Role-routing root has no running step".into());
    };
    if step_revision != revision {
        return Err("Role-routing active step belongs to a stale revision".into());
    }
    if started_at_ms.is_some_and(|started| {
        now_ms.saturating_sub(started) > settings.limits.step_timeout_ms as i64
    }) {
        return Err("Role-routing step deadline reached".into());
    }
    let budget = load_budget(connection, root_id)?;
    // The running step was reserved when it was claimed and is already present in `budget`.
    // Reserving again here makes a one-step policy reject its first and only valid dispatch.
    if budget.steps > settings.limits.max_reasoning_steps {
        return Err("Role-routing step budget exceeded".into());
    }
    if let Some(max_cost) = settings.limits.max_estimated_cost_micros {
        if budget.spent_cost_micros > max_cost {
            return Err("Role-routing cost budget exceeded".into());
        }
        let actor = settings
            .actors
            .iter()
            .find(|actor| actor.id == actor_id)
            .ok_or_else(|| "Role-routing active actor is unavailable".to_string())?;
        // Local actors have no metered provider cost. A cloud actor has no trusted price at this
        // boundary, so a configured monetary cap must fail closed instead of treating it as zero.
        if actor.location != "local" {
            return Err("Role-routing next-step cost is unavailable".into());
        }
    }
    Ok(Some(DispatchPermit {
        root_id: root_id.to_string(),
        revision: u32::try_from(step_revision).unwrap_or(0),
        step_id,
        ordinal,
        actor_id,
        purpose,
        config_fingerprint,
    }))
}

/// Returns the planned steps of a compiled recipe, used by tests and the async driver.
pub(crate) fn planned_steps(
    settings: &RoleRoutingSettings,
    recipe_id: &str,
) -> Result<Vec<PlannedStep>, String> {
    compile(settings, recipe_id).map(|recipe| recipe.steps)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::role_routing::contracts::{RoutingActor, RoutingRecipe};
    use rusqlite::params;

    fn fixture(
        limits: impl FnOnce(&mut crate::role_routing::contracts::RoutingLimits),
    ) -> (Connection, RoleRoutingSettings) {
        let connection = Connection::open_in_memory().expect("database");
        connection
            .execute_batch(
                "PRAGMA foreign_keys=ON;
                 CREATE TABLE conversations(id TEXT PRIMARY KEY);
                 CREATE TABLE runtime_runs(id TEXT PRIMARY KEY);
                 CREATE TABLE conversation_messages(id TEXT PRIMARY KEY);
                 INSERT INTO conversations VALUES('c');
                 INSERT INTO conversation_messages VALUES('m');",
            )
            .expect("base");
        crate::role_routing::schema::migrate(&connection).expect("schema");
        connection
            .execute(
                "INSERT INTO rr_policy_versions VALUES('p',1,'{}','d',1)",
                [],
            )
            .expect("policy");
        let mut settings = RoleRoutingSettings::default();
        settings.enabled = true;
        settings.actors = vec![RoutingActor {
            id: "qwen".into(),
            label: "Qwen".into(),
            aliases: vec![],
            transport: "provider".into(),
            provider_id: Some("qwen".into()),
            model: None,
            location: "local".into(),
            resource_group: "gpu".into(),
            max_input_bytes: 1024,
            capabilities: vec!["reason".into()],
        }];
        settings.roles.reasoner = Some("qwen".into());
        settings.recipes = vec![RoutingRecipe {
            id: "direct".into(),
            action: crate::role_routing::contracts::RoutingAction::Respond,
            roles: vec!["reasoner".into()],
            enabled: true,
        }];
        limits(&mut settings.limits);
        (connection, settings)
    }

    fn insert_root_and_step(connection: &Connection, status: &str, started_at_ms: i64) {
        connection
            .execute(
                "INSERT INTO rr_roots(root_id,conversation_id,policy_id,revision,phase,origin,presentation_mode,started_at_ms,deadline_at_ms,scope_digest) VALUES('r','c','p',0,'responding','text','visual',1,100000,'')",
                [],
            )
            .expect("root");
        connection
            .execute(
                "INSERT INTO rr_steps(id,root_id,revision,ordinal,actor_id,purpose,status,config_fingerprint,adapter_state_json,started_at_ms) VALUES('s','r',0,0,'qwen','respond',?1,'{}','{}',?2)",
                params![status, started_at_ms],
            )
            .expect("step");
    }

    #[test]
    fn rr_22_loop_budget_at_the_executor() {
        let (connection, settings) = fixture(|limits| {
            limits.max_reasoning_steps = 1;
        });
        insert_root_and_step(&connection, "running", 2);
        // The running step is the one already reserved slot, not a request for a second slot.
        assert!(permit_next_step(&connection, &settings, "r", 3).is_ok());
        let budget = load_budget(&connection, "r").expect("budget");
        assert_eq!(budget.steps, 1);
        connection
            .execute(
                "INSERT INTO rr_steps(id,root_id,revision,ordinal,actor_id,purpose,status,config_fingerprint,adapter_state_json,started_at_ms) VALUES('spent','r',0,1,'qwen','respond','succeeded','{}','{}',1)",
                [],
            )
            .expect("spent step");
        assert!(permit_next_step(&connection, &settings, "r", 3).is_err());
    }

    #[test]
    fn rr_22_unstarted_cancelled_step_does_not_spend_budget() {
        let (connection, settings) = fixture(|limits| {
            limits.max_reasoning_steps = 1;
        });
        insert_root_and_step(&connection, "running", 2);
        connection
            .execute(
                "INSERT INTO rr_steps(id,root_id,revision,ordinal,actor_id,purpose,status,config_fingerprint,adapter_state_json) VALUES('never-started','r',0,1,'qwen','respond','cancelled','{}','{}')",
                [],
            )
            .expect("cancelled planned step");
        assert_eq!(load_budget(&connection, "r").expect("budget").steps, 1);
        assert!(permit_next_step(&connection, &settings, "r", 3).is_ok());
    }

    #[test]
    fn rr_22_cloud_dispatch_requires_a_known_cost_under_a_cap() {
        let (connection, mut settings) = fixture(|limits| {
            limits.max_estimated_cost_micros = Some(1_000);
        });
        settings.actors[0].location = "cloud".into();
        insert_root_and_step(&connection, "running", 2);
        assert!(permit_next_step(&connection, &settings, "r", 3).is_err());
        settings.actors[0].location = "local".into();
        assert!(permit_next_step(&connection, &settings, "r", 3).is_ok());
    }

    #[test]
    fn rr_22_deadline_at_the_executor() {
        let (connection, settings) = fixture(|_| {});
        insert_root_and_step(&connection, "running", 2);
        // deadline_at_ms is 100000 from the fixture.
        assert!(permit_next_step(&connection, &settings, "r", 100_000).is_err());
        assert!(permit_next_step(&connection, &settings, "r", 3).is_ok());
    }

    #[test]
    fn rr_05_queued_root_is_not_dispatchable_yet() {
        let (connection, settings) = fixture(|_| {});
        connection
            .execute(
                "INSERT INTO rr_roots(root_id,conversation_id,policy_id,revision,phase,origin,presentation_mode,started_at_ms,scope_digest) VALUES('r','c','p',0,'queued','text','visual',1,'')",
                [],
            )
            .expect("queued root");
        assert_eq!(
            permit_next_step(&connection, &settings, "r", 2).expect("queued"),
            None
        );
    }

    #[test]
    fn rr_07_compile_reuses_the_recipe_plan() {
        let (_connection, settings) = fixture(|_| {});
        let steps = planned_steps(&settings, "direct").expect("plan");
        assert_eq!(steps.len(), 1);
        assert_eq!(steps[0].actor_id, "qwen");
        assert_eq!(steps[0].purpose, "respond");
    }
}
