//! Echo isolation: the voice contract's guarantee that TTS playback is not heard as a user,
//! while a person speaking over it still reaches ASR.
use super::{evidence, pass, CheckFuture};
use crate::diagnosis::contract::{Capability, Evidence, Outcome, Reason, Tier};
use crate::voice::audio_backend;
use crate::AppState;

const CAP: Capability = Capability::VoiceEcho;

pub(super) fn run(state: &AppState) -> CheckFuture<'_> {
    Box::pin(async move {
        let aec_enabled = state
            .sqlite_readers
            .read(crate::persistence::load_voice_settings)
            .map(|settings| settings.aec_enabled)
            .ok();
        let status = audio_backend::global().status();
        collect(
            aec_enabled,
            BackendView {
                available: status.available,
                reason: status.reason,
                capture_active: status.capture_active,
                aec_active: status.aec_active,
            },
            audio_backend::echo_contract_self_test().map(|(silenced, preserved, untouched)| {
                EchoContractCheck {
                    playback_only_silenced: silenced,
                    overlapping_speech_preserved: preserved,
                    unrelated_voice_untouched: untouched,
                }
            }),
        )
    })
}

#[derive(Clone, Copy)]
pub(super) struct EchoContractCheck {
    pub(super) playback_only_silenced: bool,
    pub(super) overlapping_speech_preserved: bool,
    pub(super) unrelated_voice_untouched: bool,
}

pub(super) struct BackendView {
    pub(super) available: bool,
    pub(super) reason: Option<String>,
    pub(super) capture_active: bool,
    pub(super) aec_active: bool,
}

pub(super) fn collect(
    aec_enabled: Option<bool>,
    backend: BackendView,
    contract: Option<EchoContractCheck>,
) -> Vec<Evidence> {
    let mut out = Vec::new();

    out.push(match aec_enabled {
        Some(true) => pass("voice.aec-setting", CAP, Tier::Static),
        Some(false) => evidence(
            "voice.aec-setting",
            CAP,
            Tier::Static,
            Outcome::Degraded,
            Reason::AecInactive,
        ),
        None => evidence(
            "voice.aec-setting",
            CAP,
            Tier::Static,
            Outcome::Unverified,
            Reason::Internal,
        ),
    });

    out.push(if !backend.available {
        evidence(
            "voice.backend",
            CAP,
            Tier::Static,
            Outcome::Fail,
            Reason::Unavailable,
        )
        .detail(backend.reason.as_deref().unwrap_or(""))
    } else if backend.capture_active && aec_enabled == Some(true) && !backend.aec_active {
        evidence(
            "voice.backend",
            CAP,
            Tier::Probe,
            Outcome::Fail,
            Reason::AecInactive,
        )
    } else if backend.capture_active {
        pass("voice.backend", CAP, Tier::Probe)
    } else {
        pass("voice.backend", CAP, Tier::Static)
    });

    if let Some(contract) = contract {
        out.push(if !contract.playback_only_silenced {
            evidence(
                "voice.echo-contract",
                CAP,
                Tier::Static,
                Outcome::Fail,
                Reason::EchoLeak,
            )
        } else if !contract.overlapping_speech_preserved || !contract.unrelated_voice_untouched {
            evidence(
                "voice.echo-contract",
                CAP,
                Tier::Static,
                Outcome::Fail,
                Reason::SpeechSuppressed,
            )
        } else {
            pass("voice.echo-contract", CAP, Tier::Static)
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn backend(available: bool, capture: bool, aec: bool) -> BackendView {
        BackendView {
            available,
            reason: None,
            capture_active: capture,
            aec_active: aec,
        }
    }

    fn held() -> EchoContractCheck {
        EchoContractCheck {
            playback_only_silenced: true,
            overlapping_speech_preserved: true,
            unrelated_voice_untouched: true,
        }
    }

    fn outcome(items: &[Evidence], source: &str) -> (Outcome, Reason) {
        let item = items.iter().find(|item| item.source == source).unwrap();
        (item.outcome, item.reason)
    }

    #[test]
    fn healthy_idle_backend_passes() {
        let items = collect(Some(true), backend(true, false, false), Some(held()));
        assert!(items.iter().all(|item| item.outcome == Outcome::Pass));
    }

    #[test]
    fn capture_without_aec_fails_only_while_capturing() {
        let items = collect(Some(true), backend(true, true, false), Some(held()));
        assert_eq!(
            outcome(&items, "voice.backend"),
            (Outcome::Fail, Reason::AecInactive)
        );
        let idle = collect(Some(true), backend(true, false, false), Some(held()));
        assert_eq!(outcome(&idle, "voice.backend").0, Outcome::Pass);
    }

    #[test]
    fn disabled_aec_degrades_with_a_settings_action_reason() {
        let items = collect(Some(false), backend(true, false, false), Some(held()));
        assert_eq!(
            outcome(&items, "voice.aec-setting"),
            (Outcome::Degraded, Reason::AecInactive)
        );
    }

    #[test]
    fn contract_breaks_are_distinguished_from_each_other() {
        let leak = EchoContractCheck {
            playback_only_silenced: false,
            ..held()
        };
        let suppressed = EchoContractCheck {
            overlapping_speech_preserved: false,
            ..held()
        };
        assert_eq!(
            outcome(
                &collect(Some(true), backend(true, false, true), Some(leak)),
                "voice.echo-contract"
            ),
            (Outcome::Fail, Reason::EchoLeak)
        );
        assert_eq!(
            outcome(
                &collect(Some(true), backend(true, false, true), Some(suppressed)),
                "voice.echo-contract"
            ),
            (Outcome::Fail, Reason::SpeechSuppressed)
        );
    }

    #[test]
    fn the_shipped_echo_handling_satisfies_the_contract() {
        if let Some((silenced, preserved, untouched)) = audio_backend::echo_contract_self_test() {
            assert!(silenced && preserved && untouched);
        }
    }
}
