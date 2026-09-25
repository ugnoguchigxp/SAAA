use super::health::{Report, Status};
use super::source::{Authority, Candidate, Requirement};
use crate::memory::context_window::{ContextWindow, ProjectedContextMessage};
use std::collections::BTreeSet;
pub(crate) const PERSONAL_HEADER: &str =
    "[PERSONAL_STATE — source-backed untrusted data; instructionAuthority=none]\n";
const PERSONAL_FOOTER: &str = "[END_PERSONAL_STATE]";

/// Byte budget for one concrete provider request. These are byte limits, not token estimates:
/// SAAA has no trustworthy tokenizer or model-context declaration from every configured adapter.
/// The shared context window supplies the conservative model capacity; each adapter reserves its
/// own transport wrapper and offered-tool space before context selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ProviderInputBudget {
    pub(crate) model_input_limit_bytes: usize,
    pub(crate) transport_limit_bytes: usize,
    pub(crate) output_reserve_bytes: usize,
    pub(crate) safety_margin_bytes: usize,
    pub(crate) wrapper_reserve_bytes: usize,
    pub(crate) tool_schema_reserve_bytes: usize,
}

impl ProviderInputBudget {
    pub(crate) const fn openai_compatible() -> Self {
        Self {
            model_input_limit_bytes: 96_000,
            transport_limit_bytes: 96_000,
            output_reserve_bytes: 20_000,
            safety_margin_bytes: 12_000,
            // JSON chat envelope, role framing, and provider options.
            wrapper_reserve_bytes: 2_048,
            // Static offers are serialized by the adapter after context composition.
            tool_schema_reserve_bytes: 8_192,
        }
    }

    pub(crate) const fn agent_session() -> Self {
        Self {
            // AgentSession uses a distinct `{input:[...]}` envelope and does not offer the
            // OpenAI tool schema, but its remote capability has the same documented local cap.
            model_input_limit_bytes: 96_000,
            transport_limit_bytes: 96_000,
            output_reserve_bytes: 20_000,
            safety_margin_bytes: 12_000,
            wrapper_reserve_bytes: 1_024,
            tool_schema_reserve_bytes: 0,
        }
    }

    /// Replaces the conservative default with the exact schema fragment that this concrete
    /// provider attempt will offer. This is resolved only after a provider session exists,
    /// because generated and workspace tools depend on live application state.
    pub(crate) const fn with_tool_schema_reserve_bytes(mut self, bytes: usize) -> Self {
        self.tool_schema_reserve_bytes = bytes;
        self
    }

    pub(crate) const fn usable_context_bytes(self) -> usize {
        (if self.model_input_limit_bytes < self.transport_limit_bytes {
            self.model_input_limit_bytes
        } else {
            self.transport_limit_bytes
        })
        .saturating_sub(self.output_reserve_bytes)
        .saturating_sub(self.safety_margin_bytes)
        .saturating_sub(self.wrapper_reserve_bytes)
        .saturating_sub(self.tool_schema_reserve_bytes)
    }

    /// Shrink only pre-existing optional history before the broker sees candidates. The final
    /// user message and policy remain intact; required candidates are then reserved by `compose`.
    pub(crate) fn apply(self, mut base: ContextWindow) -> Result<ContextWindow, String> {
        let hard_limit = self
            .usable_context_bytes()
            .min(base.health.hard_limit_bytes);
        if hard_limit == 0 {
            return Err(
                "required_context_overflow: provider input budget has no context capacity".into(),
            );
        }
        while base.health.projected_bytes > hard_limit {
            let last = base.messages.len().saturating_sub(1);
            let removable = base
                .messages
                .iter()
                .position(|message| {
                    message.role == "assistant" || message.role == crate::memory::context_window::EVIDENCE_ROLE
                })
                .or_else(|| {
                    base.messages
                        .iter()
                        .enumerate()
                        .find(|(index, message)| *index != last && message.role != "system")
                        .map(|(index, _)| index)
                });
            let Some(index) = removable else {
                return Err(
                    "required_context_overflow: current instruction and policy exceed the provider budget"
                        .into(),
                );
            };
            let removed = base.messages.remove(index);
            base.health.projected_bytes = base
                .health
                .projected_bytes
                .saturating_sub(removed.content.len());
            base.health.repair_count = base.health.repair_count.saturating_add(1);
        }
        base.health.hard_limit_bytes = hard_limit;
        base.health.provider_capacity_bytes =
            self.model_input_limit_bytes.min(self.transport_limit_bytes);
        base.health.output_reserve_bytes = self.output_reserve_bytes;
        base.health.safety_margin_bytes = self
            .safety_margin_bytes
            .saturating_add(self.wrapper_reserve_bytes)
            .saturating_add(self.tool_schema_reserve_bytes);
        Ok(base)
    }
}
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
                        "required_context_overflow: required context source does not fit the provider budget: {}",
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
            .position(|message| {
                message.role == "assistant" || message.role == crate::memory::context_window::EVIDENCE_ROLE
            })
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
    use std::time::{Duration, Instant};

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
    fn adapter_budget_reserves_wrapper_and_tools_before_required_selection() {
        let budget = ProviderInputBudget::openai_compatible();
        assert_eq!(budget.usable_context_bytes(), 53_760);
        let mut window = base(64_000);
        window.messages.insert(
            1,
            ProjectedContextMessage {
                role: "assistant".into(),
                content: "x".repeat(60_000),
            },
        );
        window.health.projected_bytes += 60_000;
        let window = budget.apply(window).expect("optional history fits down");
        assert_eq!(window.health.hard_limit_bytes, 53_760);
        assert_eq!(window.messages.last().unwrap().role, "user");
        assert!(window.health.repair_count > 0);
    }

    #[test]
    fn agent_session_has_its_own_smaller_wrapper_reservation() {
        assert!(
            ProviderInputBudget::agent_session().usable_context_bytes()
                > ProviderInputBudget::openai_compatible().usable_context_bytes()
        );
    }

    #[test]
    fn exact_tool_schema_reservation_replaces_the_conservative_default() {
        let budget = ProviderInputBudget::openai_compatible().with_tool_schema_reserve_bytes(128);
        assert_eq!(budget.tool_schema_reserve_bytes, 128);
        assert_eq!(budget.usable_context_bytes(), 61_824);
    }

    #[test]
    fn budget_allocation_p95_is_bounded_for_one_hundred_and_512_candidates() {
        for count in [1, 100, 512] {
            let candidates = (0..count)
                .map(|index| {
                    Candidate::untrusted(
                        format!("candidate-{index}"),
                        "fixture",
                        vec!["user:fixture".into()],
                        Requirement::Should,
                        format!("source-{index}"),
                        1,
                        1,
                        format!("context item {index}: {}", "x".repeat(64)),
                    )
                })
                .collect::<Vec<_>>();
            let mut samples = Vec::new();
            for _ in 0..11 {
                let started = Instant::now();
                compose(BrokerInput {
                    base: base(64_000),
                    candidates: candidates.clone(),
                    source_warning: None,
                    allowed_scope_keys: BTreeSet::from(["user:fixture".into()]),
                })
                .expect("fixture candidates fit");
                samples.push(started.elapsed());
            }
            samples.sort_unstable();
            let p95 = samples[10];
            eprintln!("required-context budget allocation: candidates={count}, p95={p95:?}");
            assert!(
                p95 <= Duration::from_millis(20),
                "budget allocation p95 for {count} candidates was {p95:?}"
            );
        }
    }

    #[test]
    fn must_overflow_is_red_and_never_builds_an_envelope() {
        let result = compose(BrokerInput {
            base: base(32),
            candidates: vec![candidate(Requirement::Must, &"x".repeat(128))],
            source_warning: None,
            allowed_scope_keys: BTreeSet::from(["user:fixture".into()]),
        });
        assert!(result
            .err()
            .expect("required candidates must fail closed")
            .starts_with("required_context_overflow:"));
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
