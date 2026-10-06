//! Pure rules that turn evidence into capability states. No I/O, no clock reads.
//!
//! - `core` evidence must all hold. Every other route is an alternative; one working route is enough.
//! - An unproven state is never green: capabilities that need live proof stay `unverified`
//!   until a fresh probe pass or the latest real use passed.
//! - Expired evidence becomes `unverified`. Advisory evidence can only degrade.
//! - Only the newest recorded real use counts, and a newer probe pass supersedes an older failed use.
use super::contract::{
    Action, Capability, CapabilityReport, CapabilityState, Evidence, Importance, Outcome, Reason,
    Tier, Weight,
};
use std::collections::BTreeMap;

fn severity(outcome: Outcome) -> u8 {
    match outcome {
        Outcome::Disabled | Outcome::Pass => 0,
        Outcome::Degraded => 1,
        Outcome::Unverified => 2,
        Outcome::Fail => 3,
    }
}

fn rank(state: CapabilityState) -> u8 {
    match state {
        CapabilityState::Unavailable => 0,
        CapabilityState::Unverified => 1,
        CapabilityState::Degraded => 2,
        CapabilityState::Ready | CapabilityState::Disabled => 3,
    }
}

fn state_of_outcome(outcome: Outcome) -> CapabilityState {
    match outcome {
        Outcome::Pass => CapabilityState::Ready,
        Outcome::Degraded => CapabilityState::Degraded,
        Outcome::Unverified => CapabilityState::Unverified,
        Outcome::Fail => CapabilityState::Unavailable,
        Outcome::Disabled => CapabilityState::Disabled,
    }
}

fn effective(evidence: &Evidence, now: u64) -> Evidence {
    let mut evidence = evidence.clone();
    if evidence.outcome != Outcome::Disabled && evidence.expires_at.is_some_and(|at| now >= at) {
        evidence.outcome = Outcome::Unverified;
        evidence.reason = Reason::Expired;
    }
    if evidence.importance == Importance::Advisory && evidence.outcome == Outcome::Fail {
        evidence.outcome = Outcome::Degraded;
    }
    evidence
}

fn counts(evidence: &Evidence) -> bool {
    evidence.outcome != Outcome::Disabled
        && !(evidence.importance == Importance::Advisory && evidence.outcome == Outcome::Unverified)
}

fn worst<'a>(group: &[&'a Evidence]) -> Option<&'a Evidence> {
    group
        .iter()
        .copied()
        .filter(|evidence| counts(evidence))
        .fold(None, |best: Option<&Evidence>, next| match best {
            Some(current) if severity(current.outcome) >= severity(next.outcome) => Some(current),
            _ => Some(next),
        })
}

pub(crate) fn capability_report(
    capability: Capability,
    evidence: &[Evidence],
    now: u64,
) -> CapabilityReport {
    let effective_all: Vec<Evidence> = evidence
        .iter()
        .filter(|item| item.capability == capability)
        .map(|item| effective(item, now))
        .collect();

    let latest_observed = effective_all
        .iter()
        .filter(|item| {
            item.tier == Tier::Observed
                && !matches!(item.outcome, Outcome::Disabled | Outcome::Unverified)
        })
        .max_by_key(|item| item.observed_at);
    let newest_probe_pass = effective_all
        .iter()
        .filter(|item| item.tier == Tier::Probe && item.outcome == Outcome::Pass)
        .map(|item| item.observed_at)
        .max();
    // A capture failure is not exercised by a provider probe, so a probe never supersedes it.
    let observed_in_effect = latest_observed.filter(|observed| {
        observed.outcome == Outcome::Pass
            || observed.reason == Reason::CaptureFailed
            || newest_probe_pass.is_none_or(|probe| probe <= observed.observed_at)
    });

    let mut groups: BTreeMap<&str, Vec<&Evidence>> = BTreeMap::new();
    // A real use that succeeded after a probe failed shows the capability works again.
    let passed_since = observed_in_effect
        .filter(|observed| observed.outcome == Outcome::Pass)
        .map(|observed| observed.observed_at);
    for item in &effective_all {
        if item.tier == Tier::Observed {
            continue;
        }
        let superseded = item.tier == Tier::Probe
            && item.outcome != Outcome::Pass
            && passed_since.is_some_and(|at| item.observed_at < at);
        if !superseded {
            groups.entry(item.route.as_str()).or_default().push(item);
        }
    }
    if let Some(observed) = observed_in_effect {
        groups.entry("core").or_default().push(observed);
    }
    let core = groups.remove("core").unwrap_or_default();
    let core_worst = worst(&core);
    let best_route = groups
        .values()
        .filter_map(|group| worst(group))
        .max_by_key(|evidence| 3 - severity(evidence.outcome).min(3));

    let deciding: Vec<&Evidence> = [core_worst, best_route].into_iter().flatten().collect();
    let mut state = deciding
        .iter()
        .map(|evidence| state_of_outcome(evidence.outcome))
        .min_by_key(|state| rank(*state));
    let mut reason = deciding
        .iter()
        .max_by_key(|evidence| severity(evidence.outcome))
        .map(|evidence| evidence.reason)
        .unwrap_or(Reason::Ok);

    let has_proof = effective_all.iter().any(|item| {
        item.outcome == Outcome::Pass
            && (item.tier == Tier::Probe
                || observed_in_effect.is_some_and(|observed| std::ptr::eq(observed, item)))
    });
    match state {
        None => {
            if effective_all
                .iter()
                .any(|item| item.outcome == Outcome::Disabled)
            {
                state = Some(CapabilityState::Disabled);
                reason = Reason::Disabled;
            } else {
                state = Some(CapabilityState::Unverified);
                reason = Reason::NotObserved;
            }
        }
        Some(CapabilityState::Ready) => {
            if capability.needs_live_proof() && !has_proof {
                state = Some(CapabilityState::Unverified);
                reason = Reason::NotProven;
            } else {
                reason = Reason::Ok;
            }
        }
        Some(_) => {}
    }
    // Every required path is switched off: recent use or advisory evidence must not revive it.
    let required_paths: Vec<&Evidence> = effective_all
        .iter()
        .filter(|item| item.tier != Tier::Observed && item.importance == Importance::Required)
        .collect();
    if !required_paths.is_empty()
        && required_paths
            .iter()
            .all(|item| item.outcome == Outcome::Disabled)
    {
        state = Some(CapabilityState::Disabled);
        reason = Reason::Disabled;
    }
    let state = state.unwrap_or(CapabilityState::Unverified);

    let verified_at = effective_all
        .iter()
        .filter(|item| {
            item.outcome == Outcome::Pass
                && (item.tier == Tier::Probe
                    || (item.tier == Tier::Observed
                        && latest_observed.is_some_and(|latest| std::ptr::eq(latest, *item))))
        })
        .map(|item| item.observed_at)
        .max();

    let mut actions = vec![Action::Retest];
    if matches!(
        reason,
        Reason::NotConfigured
            | Reason::AuthFailed
            | Reason::NotAdvertised
            | Reason::Unreachable
            | Reason::AecInactive
            | Reason::Disabled
    ) {
        actions.insert(0, Action::OpenSettings);
    }

    let mut listed = effective_all;
    listed.sort_by(|a, b| {
        severity(b.outcome)
            .cmp(&severity(a.outcome))
            .then_with(|| a.source.cmp(&b.source))
            .then_with(|| a.route.cmp(&b.route))
    });

    CapabilityReport {
        capability,
        state,
        reason,
        verified_at,
        optional: capability.weight() == Weight::Optional,
        actions,
        evidence: listed,
    }
}

/// Core outage means unavailable. Any other outage or degradation means degraded.
/// Unproven core or standard capabilities mean unverified. Only then ready.
pub(crate) fn overall(capabilities: &[CapabilityReport]) -> CapabilityState {
    let any = |predicate: &dyn Fn(&CapabilityReport) -> bool| capabilities.iter().any(predicate);
    if any(&|item| {
        item.capability.weight() == Weight::Core && item.state == CapabilityState::Unavailable
    }) {
        CapabilityState::Unavailable
    } else if any(&|item| {
        matches!(
            item.state,
            CapabilityState::Unavailable | CapabilityState::Degraded
        )
    }) {
        CapabilityState::Degraded
    } else if any(&|item| item.state == CapabilityState::Unverified && !item.optional) {
        CapabilityState::Unverified
    } else {
        CapabilityState::Ready
    }
}

pub(crate) fn all_capabilities(evidence: &[Evidence], now: u64) -> Vec<CapabilityReport> {
    Capability::ALL
        .iter()
        .map(|capability| capability_report(*capability, evidence, now))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: u64 = 1_000_000;

    fn ev(
        capability: Capability,
        route: &str,
        tier: Tier,
        outcome: Outcome,
        reason: Reason,
        at: u64,
    ) -> Evidence {
        Evidence {
            source: format!("test.{route}"),
            capability,
            route: route.to_string(),
            tier,
            importance: Importance::Required,
            outcome,
            reason,
            subject: None,
            detail: None,
            latency_ms: None,
            observed_at: at,
            expires_at: None,
        }
    }

    fn pass(capability: Capability, route: &str, tier: Tier, at: u64) -> Evidence {
        ev(capability, route, tier, Outcome::Pass, Reason::Ok, at)
    }

    fn state(capability: Capability, evidence: &[Evidence]) -> CapabilityReport {
        capability_report(capability, evidence, NOW)
    }

    #[test]
    fn configuration_alone_is_never_ready_for_live_capabilities() {
        let report = state(
            Capability::Conversation,
            &[pass(Capability::Conversation, "larm", Tier::Static, NOW)],
        );
        assert_eq!(report.state, CapabilityState::Unverified);
        assert_eq!(report.reason, Reason::NotProven);
        assert_eq!(report.verified_at, None);
    }

    #[test]
    fn configuration_alone_is_ready_for_local_capabilities() {
        let report = state(
            Capability::Storage,
            &[pass(Capability::Storage, "core", Tier::Static, NOW)],
        );
        assert_eq!(report.state, CapabilityState::Ready);
    }

    #[test]
    fn a_fresh_probe_pass_proves_a_live_capability() {
        let report = state(
            Capability::VoiceSpeak,
            &[
                pass(Capability::VoiceSpeak, "system", Tier::Static, NOW),
                pass(Capability::VoiceSpeak, "system", Tier::Probe, NOW - 5),
            ],
        );
        assert_eq!(report.state, CapabilityState::Ready);
        assert_eq!(report.verified_at, Some(NOW - 5));
    }

    #[test]
    fn expired_probe_evidence_falls_back_to_unverified() {
        let mut probe = pass(Capability::Conversation, "larm", Tier::Probe, NOW - 100);
        probe.expires_at = Some(NOW - 1);
        let report = state(Capability::Conversation, &[probe]);
        assert_eq!(report.state, CapabilityState::Unverified);
        assert_eq!(report.reason, Reason::Expired);
        assert_eq!(report.verified_at, None);
    }

    #[test]
    fn one_working_route_is_enough() {
        let report = state(
            Capability::VoiceSpeak,
            &[
                ev(
                    Capability::VoiceSpeak,
                    "larm",
                    Tier::Probe,
                    Outcome::Fail,
                    Reason::Unreachable,
                    NOW,
                ),
                pass(Capability::VoiceSpeak, "system", Tier::Probe, NOW),
            ],
        );
        assert_eq!(report.state, CapabilityState::Ready);
        assert_eq!(report.reason, Reason::Ok);
    }

    #[test]
    fn all_routes_failing_is_unavailable_with_the_cause() {
        let report = state(
            Capability::Conversation,
            &[
                ev(
                    Capability::Conversation,
                    "larm",
                    Tier::Probe,
                    Outcome::Fail,
                    Reason::Unreachable,
                    NOW,
                ),
                ev(
                    Capability::Conversation,
                    "codex",
                    Tier::Probe,
                    Outcome::Fail,
                    Reason::AuthFailed,
                    NOW,
                ),
            ],
        );
        assert_eq!(report.state, CapabilityState::Unavailable);
        assert!(report.actions.contains(&Action::OpenSettings));
    }

    #[test]
    fn core_evidence_must_all_hold() {
        let report = state(
            Capability::Storage,
            &[
                pass(Capability::Storage, "core", Tier::Static, NOW),
                ev(
                    Capability::Storage,
                    "core",
                    Tier::Static,
                    Outcome::Fail,
                    Reason::Internal,
                    NOW,
                ),
            ],
        );
        assert_eq!(report.state, CapabilityState::Unavailable);
    }

    #[test]
    fn advisory_failure_only_degrades_and_advisory_unverified_is_ignored() {
        let mut advisory = ev(
            Capability::Memory,
            "core",
            Tier::Static,
            Outcome::Fail,
            Reason::NotReady,
            NOW,
        );
        advisory.importance = Importance::Advisory;
        let report = state(
            Capability::Memory,
            &[
                pass(Capability::Memory, "core", Tier::Static, NOW),
                advisory.clone(),
            ],
        );
        assert_eq!(report.state, CapabilityState::Degraded);
        advisory.outcome = Outcome::Unverified;
        let report = state(
            Capability::Memory,
            &[
                pass(Capability::Memory, "core", Tier::Static, NOW),
                advisory,
            ],
        );
        assert_eq!(report.state, CapabilityState::Ready);
    }

    #[test]
    fn latest_real_use_failure_degrades_until_a_newer_probe_passes() {
        let failed_use = ev(
            Capability::VoiceListen,
            "core",
            Tier::Observed,
            Outcome::Degraded,
            Reason::RecentFailure,
            NOW - 50,
        );
        let configured = pass(Capability::VoiceListen, "larm", Tier::Static, NOW);
        let report = state(
            Capability::VoiceListen,
            &[failed_use.clone(), configured.clone()],
        );
        assert_eq!(report.state, CapabilityState::Degraded);
        assert_eq!(report.reason, Reason::RecentFailure);

        let newer_probe = pass(Capability::VoiceListen, "larm", Tier::Probe, NOW - 10);
        let report = state(
            Capability::VoiceListen,
            &[failed_use, configured, newer_probe],
        );
        assert_eq!(report.state, CapabilityState::Ready);
    }

    #[test]
    fn a_newer_successful_use_supersedes_an_older_failed_probe() {
        let failed_probe = ev(
            Capability::Conversation,
            "larm",
            Tier::Probe,
            Outcome::Fail,
            Reason::Unreachable,
            NOW - 600,
        );
        let configured = pass(Capability::Conversation, "larm", Tier::Static, NOW);
        let report = state(
            Capability::Conversation,
            &[failed_probe.clone(), configured.clone()],
        );
        assert_eq!(report.state, CapabilityState::Unavailable);
        let used = pass(Capability::Conversation, "core", Tier::Observed, NOW - 100);
        let report = state(Capability::Conversation, &[failed_probe, configured, used]);
        assert_eq!(report.state, CapabilityState::Ready);
    }

    #[test]
    fn only_the_newest_real_use_counts() {
        let old_failure = ev(
            Capability::Conversation,
            "core",
            Tier::Observed,
            Outcome::Degraded,
            Reason::RecentFailure,
            NOW - 500,
        );
        let new_success = pass(Capability::Conversation, "core", Tier::Observed, NOW - 5);
        let report = state(
            Capability::Conversation,
            &[
                old_failure,
                new_success,
                pass(Capability::Conversation, "larm", Tier::Static, NOW),
            ],
        );
        assert_eq!(report.state, CapabilityState::Ready);
        assert_eq!(report.verified_at, Some(NOW - 5));
    }

    #[test]
    fn disabled_paths_are_not_problems() {
        let disabled = ev(
            Capability::VoiceSpeak,
            "cloud",
            Tier::Static,
            Outcome::Disabled,
            Reason::Disabled,
            NOW,
        );
        let report = state(Capability::VoiceSpeak, std::slice::from_ref(&disabled));
        assert_eq!(report.state, CapabilityState::Disabled);
        let report = state(
            Capability::VoiceSpeak,
            &[
                disabled,
                pass(Capability::VoiceSpeak, "system", Tier::Probe, NOW),
            ],
        );
        assert_eq!(report.state, CapabilityState::Ready);
    }

    #[test]
    fn a_capability_whose_required_paths_are_all_off_stays_disabled() {
        let mut advisory = pass(Capability::Memory, "core", Tier::Static, NOW);
        advisory.importance = Importance::Advisory;
        let off = ev(
            Capability::Memory,
            "core",
            Tier::Static,
            Outcome::Disabled,
            Reason::Disabled,
            NOW,
        );
        let report = state(Capability::Memory, &[off, advisory]);
        assert_eq!(report.state, CapabilityState::Disabled);
        // Recent use of a path the user has since turned off does not revive it.
        let off = ev(
            Capability::VoiceListen,
            "larm",
            Tier::Static,
            Outcome::Disabled,
            Reason::Disabled,
            NOW,
        );
        let used = pass(Capability::VoiceListen, "core", Tier::Observed, NOW - 5);
        assert_eq!(
            state(Capability::VoiceListen, &[off, used]).state,
            CapabilityState::Disabled
        );
    }

    #[test]
    fn a_probe_pass_never_hides_a_microphone_capture_failure() {
        let capture = ev(
            Capability::VoiceListen,
            "core",
            Tier::Observed,
            Outcome::Fail,
            Reason::CaptureFailed,
            NOW - 50,
        );
        let upload_probe = pass(
            Capability::VoiceListen,
            "provider:asr",
            Tier::Probe,
            NOW - 10,
        );
        let report = state(Capability::VoiceListen, &[capture, upload_probe]);
        assert_eq!(report.state, CapabilityState::Unavailable);
        assert_eq!(report.reason, Reason::CaptureFailed);
    }

    #[test]
    fn no_evidence_is_unverified_not_ready() {
        let report = state(Capability::Memory, &[]);
        assert_eq!(report.state, CapabilityState::Unverified);
        assert_eq!(report.reason, Reason::NotObserved);
    }

    fn report(capability: Capability, state: CapabilityState) -> CapabilityReport {
        CapabilityReport {
            capability,
            state,
            reason: Reason::Ok,
            verified_at: None,
            optional: capability.weight() == Weight::Optional,
            actions: vec![],
            evidence: vec![],
        }
    }

    #[test]
    fn overall_follows_weight_and_never_hides_unproven_state() {
        use CapabilityState::*;
        let base = |states: [CapabilityState; 3]| {
            vec![
                report(Capability::Storage, states[0]),
                report(Capability::Conversation, states[1]),
                report(Capability::VoiceListen, states[2]),
                report(Capability::Coding, Unverified),
            ]
        };
        assert_eq!(overall(&base([Ready, Ready, Ready])), Ready);
        assert_eq!(overall(&base([Ready, Unverified, Ready])), Unverified);
        assert_eq!(overall(&base([Ready, Ready, Unverified])), Unverified);
        assert_eq!(overall(&base([Ready, Ready, Unavailable])), Degraded);
        assert_eq!(overall(&base([Ready, Degraded, Ready])), Degraded);
        assert_eq!(overall(&base([Ready, Unavailable, Ready])), Unavailable);
        assert_eq!(overall(&base([Unavailable, Ready, Ready])), Unavailable);
        assert_eq!(overall(&base([Ready, Disabled, Ready])), Ready);
    }

    #[test]
    fn empty_state_is_unverified_overall() {
        assert_eq!(
            overall(&all_capabilities(&[], NOW)),
            CapabilityState::Unverified
        );
    }
}
