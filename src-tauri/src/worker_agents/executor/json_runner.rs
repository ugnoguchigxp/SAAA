//! `compose_system` and the generic one-shot JSON runner (`OutputKind::JsonV1`).
use crate::worker_agents::contracts::*;
use async_trait::async_trait;
use std::time::Duration;

/// The model system string of a worker: the pinned context, then each pinned skill in order under
/// a fixed heading. It is built only from the immutable revision: conversation history, memory
/// and credentials never reach this module and must never be added here.
pub(crate) fn compose_system(revision: &LoadedRevision) -> String {
    let mut system = revision.system_context.clone();
    for skill in &revision.skills {
        system.push_str("\n\n## Skill: ");
        system.push_str(&skill.name);
        system.push('\n');
        system.push_str(&skill.body);
    }
    system
}

/// Single model call, no tools: the delegated input in, one JSON object out.
pub(crate) struct JsonRunner;

#[async_trait]
impl AttemptRunner for JsonRunner {
    async fn run(&self, env: &AttemptEnv<'_>) -> Result<WorkerOutput, AttemptError> {
        if env.cancellation.is_cancelled() {
            return Err(AttemptError::Cancelled);
        }
        let timeout = env
            .deadline
            .saturating_duration_since(tokio::time::Instant::now());
        if timeout <= Duration::ZERO {
            return Err(AttemptError::DeadlineExceeded);
        }
        let system = compose_system(env.revision);
        let text = env
            .model
            .complete(
                env.route,
                &system,
                &env.input.to_string(),
                env.cancellation,
                timeout,
            )
            .await
            .map_err(AttemptError::Transport)?;
        parse_single_object(&text).map(WorkerOutput::JsonV1)
    }
}

fn parse_single_object(text: &str) -> Result<serde_json::Value, AttemptError> {
    let trimmed = text.trim();
    let unfenced = trimmed
        .strip_prefix("```json")
        .or_else(|| trimmed.strip_prefix("```"))
        .and_then(|rest| rest.trim_end().strip_suffix("```"))
        .map(str::trim)
        .unwrap_or(trimmed);
    match serde_json::from_str::<serde_json::Value>(unfenced) {
        Ok(value) if value.is_object() => Ok(value),
        Ok(_) => Err(AttemptError::InvalidOutput("not a JSON object".into())),
        Err(error) => Err(AttemptError::InvalidOutput(format!(
            "not valid JSON: {error}"
        ))),
    }
}
