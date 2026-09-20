use super::health::{Report, Status};
use super::source::{Authority, Candidate, Requirement};
use crate::memory::context_window::{ContextWindow, ProjectedContextMessage};
use std::collections::BTreeSet;
pub(crate) const PERSONAL_HEADER: &str =
    "[PERSONAL_STATE — source-backed untrusted data; instructionAuthority=none]\n";
const PERSONAL_FOOTER: &str = "[END_PERSONAL_STATE]";
#[derive(Clone)]
pub(crate) struct BrokerInput {
    pub(crate) base: ContextWindow,
    pub(crate) candidates: Vec<Candidate>,
    pub(crate) source_warning: Option<String>,
    pub(crate) allowed_scope_keys: BTreeSet<String>,
}

pub(crate) struct Envelope {
    pub(crate) messages: Vec<ProjectedContextMessage>,
    pub(crate) context_health: crate::memory::context_window::ContextHealthReport,
    pub(crate) health: Report,
    pub(crate) selected: Vec<Candidate>,
    pub(crate) omitted: Vec<Candidate>,
    /// The combined personal block that was inserted before the current instruction, if any. The
    /// World send-body path uses this exact rendering instead of guessing it from message order.
    pub(crate) combined_block: Option<String>,
}

pub(crate) fn compose(mut input: BrokerInput) -> Result<Envelope, String> {
    let mut status = if input.base.health.status == "yellow" {
        Status::Yellow
    } else {
        Status::Green
    };
    let mut reasons = Vec::new();
    if let Some(warning) = input.source_warning {
        status = Status::Yellow;
        reasons.push(warning);
    }
    for candidate in &mut input.candidates {
        candidate.requirement = super::required::requirement(candidate);
    }
    // The base window contains optional history and old memory projections.  Reserve room for
    // required state before selecting it; otherwise a long history could consume the budget
    // before the safety-critical context is considered.
    reserve_required_budget(&mut input.base, &input.candidates)?;
    input.candidates.sort_by(|left, right| {
        left.requirement
            .cmp(&right.requirement)
            .then_with(|| right.utility.cmp(&left.utility))
            .then_with(|| left.candidate_id.cmp(&right.candidate_id))
    });
    let mut selected = Vec::new();
    let mut omitted = Vec::new();
    let mut seen = BTreeSet::new();
    let mut used = input.base.health.projected_bytes;
    let hard = input.base.health.hard_limit_bytes;
    let wrapper = PERSONAL_HEADER.len() + PERSONAL_FOOTER.len();
    for candidate in input.candidates {
        if candidate.authority != Authority::UntrustedData {
            return Err("Context source attempted to inject instruction authority".into());
        }
        if !candidate.scope_refs.is_empty()
            && !candidate
                .scope_refs
                .iter()
                .any(|scope| input.allowed_scope_keys.contains(scope))
        {
            return Err("Context source does not belong to the resolved scope".into());
        }
        let identity = (
            candidate.source_id.clone(),
            candidate.source_version,
            candidate.source_digest.clone(),
        );
        if !seen.insert(identity) {
            continue;
        }
        let separator = usize::from(!selected.is_empty());
        let next = used
            .saturating_add(if selected.is_empty() { wrapper } else { 0 })
            .saturating_add(separator)
            .saturating_add(candidate.cost_bytes);
        if next > hard {
            match candidate.requirement {
                Requirement::Must => {
                    return Err(format!(
                        "Required context source does not fit the provider budget: {}",
                        candidate.source_kind
                    ));
                }
                Requirement::Should => {
                    status = Status::Yellow;
                    reasons.push(format!("{}-budget-omitted", candidate.source_kind));
                }
                Requirement::May => {}
            }
            omitted.push(candidate);
            continue;
        }
        used = next;
        selected.push(candidate);
    }
    let mut combined_block = None;
    if !selected.is_empty() {
        let content = format!(
            "{PERSONAL_HEADER}{}{PERSONAL_FOOTER}",
            selected
                .iter()
                .map(|candidate| candidate.content.as_str())
                .collect::<Vec<_>>()
                .join("\n")
        );
        let current = input
            .base
            .messages
            .pop()
            .filter(|message| message.role == "user")
            .ok_or_else(|| "Context envelope lost the current instruction".to_string())?;
        input.base.messages.push(ProjectedContextMessage {
            role: "assistant".into(),
            content: content.clone(),
        });
        input.base.messages.push(current);
        combined_block = Some(content);
    }
    let current_count = input
        .base
        .messages
        .iter()
        .filter(|message| message.role == "user")
        .count();
    if current_count != 1 || used > hard {
        return Err("Context envelope invariant failed".into());
    }
    input.base.health.projected_bytes = used;
    input.base.health.current_instruction_count = current_count;
    input.base.health.status = status.as_str();
    Ok(Envelope {
        messages: input.base.messages,
        context_health: input.base.health,
        health: Report {
            status,
            reason_codes: reasons,
            projected_bytes: used,
            hard_limit_bytes: hard,
            selected_sources: selected.len(),
            omitted_sources: omitted.len(),
        },
        selected,
        omitted,
        combined_block,
    })
}

fn reserve_required_budget(
    base: &mut ContextWindow,
    candidates: &[Candidate],
) -> Result<(), String> {
    let required_bytes = candidates
        .iter()
        .filter(|candidate| candidate.requirement == Requirement::Must)
        .fold(0_usize, |used, candidate| {
            used.saturating_add(candidate.cost_bytes)
        })
        .saturating_add(
            usize::from(
                candidates
                    .iter()
                    .any(|candidate| candidate.requirement == Requirement::Must),
            ) * (PERSONAL_HEADER.len() + PERSONAL_FOOTER.len()),
        );
    if required_bytes > base.health.hard_limit_bytes {
        return Err(
            "required_context_overflow: required context exceeds the provider budget".into(),
        );
    }
    while base.health.projected_bytes.saturating_add(required_bytes) > base.health.hard_limit_bytes
    {
        let Some(index) = base
            .messages
            .iter()
            .position(|message| message.role == "assistant")
        else {
            return Err("required_context_overflow: required context cannot fit with current input and policy".into());
        };
        let removed = base.messages.remove(index);
        base.health.projected_bytes = base
            .health
            .projected_bytes
            .saturating_sub(removed.content.len());
        base.health.repair_count = base.health.repair_count.saturating_add(1);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::context_window::{ContextHealthReport, ProjectedContextMessage};
    use crate::runtime::context::source::{Candidate, Requirement};

    fn base(limit: usize) -> ContextWindow {
        ContextWindow {
            messages: vec![
                ProjectedContextMessage {
                    role: "system".into(),
                    content: "policy".into(),
                },
                ProjectedContextMessage {
                    role: "user".into(),
                    content: "current".into(),
                },
            ],
            continuity_groups: Vec::new(),
            health: ContextHealthReport {
                status: "green",
                hard_limit_bytes: limit,
                provider_capacity_bytes: limit + 32_000,
                output_reserve_bytes: 20_000,
                safety_margin_bytes: 12_000,
                projected_bytes: 13,
                loaded_source_messages: 1,
                source_history_truncated: false,
                recent_source_messages: 0,
                continuity_group_count: 0,
                continuity_source_messages: 0,
                memory_item_count: 0,
                omitted_memory_items: 0,
                omitted_loaded_source_messages: 0,
                current_instruction_count: 1,
                repair_count: 0,
            },
        }
    }

    fn candidate(requirement: Requirement, content: &str) -> Candidate {
        Candidate::untrusted(
            format!("candidate-{content}"),
            "fixture",
            vec!["user:fixture".into()],
            requirement,
            format!("source-{content}"),
            1,
            1,
            content.into(),
        )
    }

    #[test]
    fn must_overflow_is_red_and_never_builds_an_envelope() {
        let result = compose(BrokerInput {
            base: base(32),
            candidates: vec![candidate(Requirement::Must, &"x".repeat(128))],
            source_warning: None,
            allowed_scope_keys: BTreeSet::from(["user:fixture".into()]),
        });
        assert!(result.is_err());
    }

    #[test]
    fn should_overflow_degrades_without_dropping_current_instruction() {
        let envelope = compose(BrokerInput {
            base: base(32),
            candidates: vec![candidate(Requirement::Should, &"x".repeat(128))],
            source_warning: None,
            allowed_scope_keys: BTreeSet::from(["user:fixture".into()]),
        })
        .unwrap();
        assert_eq!(envelope.health.status, Status::Yellow);
        assert_eq!(envelope.messages.last().unwrap().role, "user");
        assert_eq!(envelope.context_health.current_instruction_count, 1);
    }

    #[test]
    fn candidate_from_another_scope_is_rejected() {
        let result = compose(BrokerInput {
            base: base(1_024),
            candidates: vec![candidate(Requirement::Should, "private")],
            source_warning: None,
            allowed_scope_keys: BTreeSet::from(["project:other".into()]),
        });
        assert!(result
            .err()
            .unwrap()
            .contains("does not belong to the resolved scope"));
    }

    #[test]
    fn required_state_evicts_optional_history_before_it_overflows() {
        let mut window = base(220);
        window.messages.insert(
            1,
            ProjectedContextMessage {
                role: "assistant".into(),
                content: "x".repeat(170),
            },
        );
        window.health.projected_bytes += 170;
        let required = Candidate::untrusted(
            "state".into(),
            "personal-state",
            vec!["user:fixture".into()],
            Requirement::Should,
            "state".into(),
            1,
            1,
            r#"{"status":"Active","value":"keep"}"#.into(),
        );
        let envelope = compose(BrokerInput {
            base: window,
            candidates: vec![required],
            source_warning: None,
            allowed_scope_keys: BTreeSet::from(["user:fixture".into()]),
        })
        .unwrap();
        assert_eq!(envelope.selected.len(), 1);
        assert_eq!(envelope.selected[0].requirement, Requirement::Must);
        assert!(envelope.combined_block.unwrap().contains("keep"));
    }

    #[test]
    fn required_only_overflow_has_a_stable_reason_code() {
        let result = compose(BrokerInput {
            base: base(32),
            candidates: vec![Candidate::untrusted(
                "state".into(),
                "personal-state",
                vec!["user:fixture".into()],
                Requirement::Should,
                "state".into(),
                1,
                1,
                format!(r#"{{"status":"Active","value":"{}"}}"#, "x".repeat(128)),
            )],
            source_warning: None,
            allowed_scope_keys: BTreeSet::from(["user:fixture".into()]),
        });
        assert!(matches!(result, Err(error) if error.contains("required_context_overflow")));
    }
}
