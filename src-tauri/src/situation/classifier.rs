use super::contracts::{
    AudioState, CalendarState, CalibrationParameters, ConversationState, Evidence,
    ForegroundCategory, MicrophoneState, ShadowDecision, SignalSnapshot, SituationState,
    POLICY_VERSION, RULE_VERSION,
};

#[derive(Debug, Clone)]
pub struct Candidate {
    pub scene: String,
    pub confidence: u8,
    pub user_attention: String,
    pub audio_environment: String,
    pub evidence: Vec<Evidence>,
    pub explicit: bool,
}

#[derive(Debug, Clone)]
pub struct Hysteresis {
    stable: SituationState,
    candidate_scene: String,
    candidate_since: String,
    candidate_count: u8,
    exit_count: u8,
    last_transition_ms: u128,
}

impl Hysteresis {
    #[cfg(test)]
    pub fn new(now: &str) -> Self {
        Self {
            stable: super::contracts::initial_state(now),
            candidate_scene: "UNKNOWN".to_string(),
            candidate_since: now.to_string(),
            candidate_count: 0,
            exit_count: 0,
            last_transition_ms: 0,
        }
    }

    pub fn from_state(state: SituationState) -> Self {
        Self {
            candidate_scene: state.scene.clone(),
            candidate_since: state.candidate_since.clone(),
            stable: state,
            candidate_count: 0,
            exit_count: 0,
            last_transition_ms: 0,
        }
    }

    #[cfg(test)]
    pub fn update(
        &mut self,
        candidate: &Candidate,
        now: &str,
        now_ms: u128,
    ) -> (SituationState, bool) {
        self.update_with_parameters(candidate, now, now_ms, &CalibrationParameters::default())
    }

    pub fn update_with_parameters(
        &mut self,
        candidate: &Candidate,
        now: &str,
        now_ms: u128,
        parameters: &CalibrationParameters,
    ) -> (SituationState, bool) {
        if candidate.scene != self.candidate_scene {
            self.candidate_scene = candidate.scene.clone();
            self.candidate_since = now.to_string();
            self.candidate_count = 1;
            self.exit_count = 0;
        } else {
            self.candidate_count = self.candidate_count.saturating_add(1);
        }

        let same_as_stable = candidate.scene == self.stable.scene;
        let low_confidence =
            candidate.confidence <= parameters.low_confidence_max || candidate.scene == "UNKNOWN";
        if same_as_stable {
            self.exit_count = 0;
            self.stable.confidence = candidate.confidence;
            self.stable
                .user_attention
                .clone_from(&candidate.user_attention);
            self.stable
                .audio_environment
                .clone_from(&candidate.audio_environment);
            self.stable.evidence.clone_from(&candidate.evidence);
            self.stable
                .candidate_since
                .clone_from(&self.candidate_since);
            self.stable.updated_at = now.to_string();
            return (self.stable.clone(), false);
        }

        if low_confidence && self.stable.scene != "UNKNOWN" {
            self.exit_count = self.exit_count.saturating_add(1);
        } else {
            self.exit_count = 0;
        }

        let cooldown_elapsed = self.last_transition_ms == 0
            || now_ms.saturating_sub(self.last_transition_ms) >= u128::from(parameters.cooldown_ms);
        let should_enter = candidate.explicit
            || (candidate.confidence >= parameters.classification_min_confidence
                && self.candidate_count >= parameters.enter_sample_count
                && cooldown_elapsed);
        let should_exit =
            low_confidence && self.exit_count >= parameters.exit_sample_count && cooldown_elapsed;
        if should_enter || should_exit {
            self.stable = SituationState {
                scene: if should_exit {
                    "UNKNOWN".to_string()
                } else {
                    candidate.scene.clone()
                },
                confidence: if should_exit {
                    candidate.confidence.min(parameters.low_confidence_max)
                } else {
                    candidate.confidence
                },
                user_attention: candidate.user_attention.clone(),
                audio_environment: candidate.audio_environment.clone(),
                evidence: candidate.evidence.clone(),
                candidate_since: self.candidate_since.clone(),
                stable_since: now.to_string(),
                updated_at: now.to_string(),
                rule_version: RULE_VERSION.to_string(),
            };
            self.last_transition_ms = now_ms;
            self.exit_count = 0;
            return (self.stable.clone(), true);
        }

        let mut projected = self.stable.clone();
        projected.candidate_since.clone_from(&self.candidate_since);
        projected.updated_at = now.to_string();
        (projected, false)
    }
}

#[cfg(test)]
pub fn classify(signals: &SignalSnapshot) -> Candidate {
    classify_with_parameters(signals, &CalibrationParameters::default())
}

pub(crate) fn classify_with_parameters(
    signals: &SignalSnapshot,
    parameters: &CalibrationParameters,
) -> Candidate {
    let mut scores: Vec<(&str, i32, Vec<Evidence>)> = Vec::new();
    let mut explicit = false;

    let conversation = match signals.conversation.state {
        ConversationState::UserInput => Some((100, "explicit-user-input")),
        ConversationState::ModelRunning => Some((100, "model-run-active")),
        ConversationState::AgentRunning => Some((100, "agent-run-active")),
        ConversationState::Idle => None,
    };
    let microphone = match signals.microphone.state {
        MicrophoneState::SaaaCapturing => Some((100, "saaa-capture-active")),
        MicrophoneState::SaaaTranscribing => Some((100, "saaa-transcription-active")),
        _ => None,
    };
    if let Some((weight, code)) = conversation.or(microphone) {
        explicit = true;
        scores.push((
            "CONVERSATION",
            weight,
            vec![Evidence {
                code: code.to_string(),
                weight,
            }],
        ));
    }

    if signals.foreground.category == ForegroundCategory::Sensitive {
        return Candidate {
            scene: "UNKNOWN".to_string(),
            confidence: 20,
            user_attention: "busy".to_string(),
            audio_environment: audio_environment(signals),
            evidence: vec![Evidence {
                code: "sensitive-application".to_string(),
                weight: 20,
            }],
            explicit: false,
        };
    }

    if signals.foreground.category == ForegroundCategory::Communication {
        let mut score = 45;
        let mut evidence = vec![Evidence {
            code: "communication-app".to_string(),
            weight: 45,
        }];
        if signals.calendar.state == CalendarState::MeetingLikely {
            score += 35;
            evidence.push(Evidence {
                code: "calendar-meeting-likely".to_string(),
                weight: 35,
            });
        }
        if signals.microphone.state == MicrophoneState::ExternalActive {
            score += 25;
            evidence.push(Evidence {
                code: "external-microphone-active".to_string(),
                weight: 25,
            });
        }
        scores.push(("MEETING", score.min(100), evidence));
    }

    match signals.foreground.category {
        ForegroundCategory::Coding => {
            scores.push((
                "CODING",
                80,
                vec![Evidence {
                    code: "coding-app".to_string(),
                    weight: 80,
                }],
            ));
            if signals.calendar.state == CalendarState::Busy {
                scores.push((
                    "FOCUS",
                    85,
                    vec![
                        Evidence {
                            code: "coding-app".to_string(),
                            weight: 55,
                        },
                        Evidence {
                            code: "calendar-busy".to_string(),
                            weight: 30,
                        },
                    ],
                ));
            }
        }
        ForegroundCategory::Writing => {
            scores.push((
                "WRITING",
                78,
                vec![Evidence {
                    code: "writing-app".to_string(),
                    weight: 78,
                }],
            ));
            if signals.calendar.state == CalendarState::Busy {
                scores.push((
                    "FOCUS",
                    83,
                    vec![
                        Evidence {
                            code: "writing-app".to_string(),
                            weight: 53,
                        },
                        Evidence {
                            code: "calendar-busy".to_string(),
                            weight: 30,
                        },
                    ],
                ));
            }
        }
        ForegroundCategory::Media => scores.push((
            "MEDIA",
            75,
            vec![Evidence {
                code: "media-app".to_string(),
                weight: 75,
            }],
        )),
        ForegroundCategory::Other | ForegroundCategory::Browser => scores.push((
            "SOLO",
            70,
            vec![Evidence {
                code: "foreground-app-available".to_string(),
                weight: 70,
            }],
        )),
        _ => {}
    }
    if signals.audio.state == AudioState::ExternalMedia {
        scores.push((
            "MEDIA",
            80,
            vec![Evidence {
                code: "external-media-active".to_string(),
                weight: 80,
            }],
        ));
    }

    let priority = [
        "CONVERSATION",
        "MEETING",
        "CODING",
        "WRITING",
        "MEDIA",
        "FOCUS",
        "SOLO",
        "UNKNOWN",
    ];
    scores.sort_by(|left, right| {
        right.1.cmp(&left.1).then_with(|| {
            priority
                .iter()
                .position(|scene| scene == &left.0)
                .cmp(&priority.iter().position(|scene| scene == &right.0))
        })
    });
    let (scene, score, evidence) = scores
        .into_iter()
        .next()
        .unwrap_or(("UNKNOWN", 0, Vec::new()));
    let scene = if score >= i32::from(parameters.classification_min_confidence) {
        scene
    } else {
        "UNKNOWN"
    };
    Candidate {
        scene: scene.to_string(),
        confidence: score.clamp(0, 100) as u8,
        user_attention: if scene == "MEETING"
            || scene == "FOCUS"
            || signals.calendar.state == CalendarState::Busy
        {
            "busy".to_string()
        } else if scene == "UNKNOWN" {
            "unknown".to_string()
        } else {
            "available".to_string()
        },
        audio_environment: audio_environment(signals),
        evidence,
        explicit,
    }
}

pub fn shadow_policy(
    state: &SituationState,
    signals: &SignalSnapshot,
    now: &str,
) -> ShadowDecision {
    let active_input = signals.conversation.state != ConversationState::Idle
        || matches!(
            signals.microphone.state,
            MicrophoneState::SaaaCapturing | MicrophoneState::SaaaTranscribing
        );
    let (attention, reason) = if signals.foreground.category == ForegroundCategory::Sensitive {
        ("IGNORE", "sensitive-safe-default")
    } else if active_input {
        ("RESPOND", "explicit-saaa-interaction")
    } else if state.scene == "UNKNOWN" || state.confidence < 45 {
        ("IGNORE", "insufficient-signal")
    } else if state.scene == "MEETING" || state.user_attention == "busy" {
        ("OBSERVE", "user-busy")
    } else if state.confidence >= 70 && state.user_attention == "available" {
        ("SUGGEST", "high-confidence-available")
    } else {
        ("OBSERVE", "passive-observation")
    };
    ShadowDecision {
        mode: "shadow".to_string(),
        proposed_attention: attention.to_string(),
        actual_execution: "NONE".to_string(),
        actual_presentation: "SILENT".to_string(),
        reason_codes: vec![reason.to_string()],
        decided_at: now.to_string(),
        policy_version: POLICY_VERSION.to_string(),
    }
}

fn audio_environment(signals: &SignalSnapshot) -> String {
    match (&signals.microphone.state, &signals.audio.state) {
        (
            MicrophoneState::SaaaCapturing
            | MicrophoneState::SaaaTranscribing
            | MicrophoneState::ExternalActive,
            _,
        ) => "speech",
        (_, AudioState::ExternalMedia | AudioState::SaaaSpeaking) => "media",
        (MicrophoneState::Inactive, AudioState::Silent) => "silence",
        _ => "unknown",
    }
    .to_string()
}
