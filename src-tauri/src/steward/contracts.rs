//! Durable, host-validated contracts for delegated work.
//!
//! These values deliberately describe authority separately from the text which
//! proposed it.  A prompt is never an authority boundary.
use serde::{Deserialize, Serialize};

pub(crate) const MAX_ACTIVE_GOALS: usize = 8;
pub(crate) const MAX_STEPS_PER_GOAL: usize = 16;
pub(crate) const MAX_REPLANS: u8 = 2;

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct TaskPlan {
    pub steps: Vec<PlanStep>,
    #[serde(default)]
    pub max_replans: u8,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct PlanStep {
    pub id: String,
    #[serde(default)]
    pub depends_on: Vec<String>,
    pub verifier: Verifier,
    #[serde(default)]
    pub recipe: Option<String>,
    #[serde(default)]
    pub capability: Option<String>,
    #[serde(default)]
    pub verifier_input: Option<String>,
}

impl TaskPlan {
    pub(crate) fn validate(&self) -> Result<(), &'static str> {
        if self.steps.is_empty()
            || self.steps.len() > MAX_STEPS_PER_GOAL
            || self.max_replans > MAX_REPLANS
        {
            return Err("work_plan_invalid");
        }
        let mut seen = std::collections::BTreeSet::new();
        for step in &self.steps {
            if step.id.is_empty() || !seen.insert(step.id.as_str()) {
                return Err("work_plan_duplicate_step");
            }
            if step.depends_on.iter().any(|dependency| {
                dependency == &step.id
                    || !self
                        .steps
                        .iter()
                        .any(|candidate| candidate.id == *dependency)
            }) {
                return Err("work_plan_invalid");
            }
        }
        // The small bounded graph admits a simple reachability check. This
        // rejects indirect cycles as well as self-dependencies.
        for step in &self.steps {
            if reaches(&self.steps, &step.id, &step.id, &mut Vec::new()) {
                return Err("work_plan_cycle");
            }
        }
        Ok(())
    }
}

fn reaches(steps: &[PlanStep], from: &str, target: &str, seen: &mut Vec<String>) -> bool {
    let Some(step) = steps.iter().find(|step| step.id == from) else {
        return false;
    };
    for dependency in &step.depends_on {
        if dependency == target
            || (!seen.contains(dependency) && {
                seen.push(dependency.clone());
                let found = reaches(steps, dependency, target, seen);
                seen.pop();
                found
            })
        {
            return true;
        }
    }
    false
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Operation {
    Read,
    TestRun,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct GoalProposal {
    pub source_message_id: String,
    pub workspace_id: String,
    pub summary: String,
    pub success_condition: Verifier,
    pub operations: Vec<Operation>,
    pub budget_runs: u8,
    pub budget_ms: u64,
    #[serde(default = "default_notify")]
    pub notify: Notify,
    #[serde(default)]
    pub quote_start: Option<u32>,
    #[serde(default)]
    pub quote_end: Option<u32>,
    #[serde(default)]
    pub target: Option<String>,
    #[serde(default)]
    pub recipe_id: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Verifier {
    TestReportObtained,
    TestsPass,
    UserConfirmationRequired,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Notify {
    Both,
    Silent,
    Speak,
}

fn default_notify() -> Notify {
    Notify::Both
}

impl GoalProposal {
    pub(crate) fn validate(&self) -> Result<(), &'static str> {
        if self.source_message_id.is_empty()
            || self.workspace_id.is_empty()
            || self.summary.trim().is_empty()
            || self.summary.chars().count() > 2_000
            || self.operations.is_empty()
            || self.operations.len() > 2
            || self.budget_runs == 0
            || self.budget_runs > 16
            || self.budget_ms == 0
            || self.budget_ms > 3_600_000
        {
            return Err("work_proposal_invalid");
        }
        Ok(())
    }

    pub(crate) fn operations_key(&self) -> &'static str {
        match self.operations.as_slice() {
            [Operation::Read] => "read",
            [Operation::TestRun] => "test_run",
            [Operation::Read, Operation::TestRun] | [Operation::TestRun, Operation::Read] => {
                "read_test"
            }
            _ => "invalid",
        }
    }

    pub(crate) fn verifier_key(&self) -> &'static str {
        match &self.success_condition {
            Verifier::TestReportObtained => "test_report_obtained",
            Verifier::TestsPass => "tests_pass",
            Verifier::UserConfirmationRequired => "user_confirmation_required",
        }
    }

    pub(crate) fn notify_key(&self) -> &'static str {
        match &self.notify {
            Notify::Both => "both",
            Notify::Silent => "silent",
            Notify::Speak => "speak",
        }
    }
}
