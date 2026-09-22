fn signal_health(signals: &SignalSnapshot) -> Vec<SignalHealthEntry> {
    vec![
        SignalHealthEntry {
            source: "foreground".to_string(),
            health: signals.foreground.health,
        },
        SignalHealthEntry {
            source: "microphone".to_string(),
            health: signals.microphone.health,
        },
        SignalHealthEntry {
            source: "audio".to_string(),
            health: signals.audio.health,
        },
        SignalHealthEntry {
            source: "calendar".to_string(),
            health: signals.calendar.health,
        },
        SignalHealthEntry {
            source: "input-activity".to_string(),
            health: signals.input_activity.health,
        },
    ]
}
fn push_event(inner: &mut RuntimeInner, event: SituationEvent) {
    let revision = inner.next_revision;
    inner.next_revision = inner.next_revision.saturating_add(1);
    inner.events.push_back((revision, event));
    while inner.events.len() > MAX_EVENTS {
        inner.events.pop_front();
    }
}
fn epoch_millis() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}
fn fresh_owned(
    input: &OwnedSignalInput,
    updated_ms: u128,
    now_ms: u128,
    sample_interval_ms: u64,
) -> OwnedSignalInput {
    let fresh_for = u128::from(sample_interval_ms).saturating_mul(3);
    if updated_ms > 0 && now_ms.saturating_sub(updated_ms) <= fresh_for {
        input.clone()
    } else {
        OwnedSignalInput {
            conversation_state: ConversationState::Idle,
            microphone_state: MicrophoneState::Inactive,
            audio_state: AudioState::Silent,
        }
    }
}
