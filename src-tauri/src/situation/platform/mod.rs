use super::contracts::{
    CalendarSignal, CalendarState, CalibrationParameters, ForegroundCategory, ForegroundSignal,
    InputActivitySignal, SignalHealth, TimeBucket,
};

#[cfg(target_os = "macos")]
mod macos;
#[cfg(not(target_os = "macos"))]
mod unsupported;

pub fn foreground_signal() -> ForegroundSignal {
    #[cfg(target_os = "macos")]
    {
        macos::foreground_signal()
    }
    #[cfg(not(target_os = "macos"))]
    {
        unsupported::foreground_signal()
    }
}

pub fn calendar_signal(enabled: bool) -> CalendarSignal {
    CalendarSignal {
        state: CalendarState::Unavailable,
        time_bucket: TimeBucket::None,
        health: if enabled {
            SignalHealth::Unsupported
        } else {
            SignalHealth::Disabled
        },
    }
}

pub fn input_activity_signal(parameters: &CalibrationParameters) -> InputActivitySignal {
    #[cfg(target_os = "macos")]
    {
        macos::input_activity_signal(parameters)
    }
    #[cfg(not(target_os = "macos"))]
    {
        unsupported::input_activity_signal(parameters)
    }
}

fn classify_input_activity_seconds(
    seconds: f64,
    parameters: &CalibrationParameters,
) -> InputActivitySignal {
    if !seconds.is_finite() || seconds < 0.0 || seconds > u64::MAX as f64 / 1_000.0 {
        return InputActivitySignal {
            state: super::contracts::InputActivityState::Unknown,
            health: SignalHealth::Degraded,
        };
    }
    let elapsed_ms = (seconds * 1_000.0).floor() as u64;
    let state = if elapsed_ms <= parameters.input_active_max_ms {
        super::contracts::InputActivityState::Active
    } else if elapsed_ms <= parameters.input_recent_max_ms {
        super::contracts::InputActivityState::Recent
    } else {
        super::contracts::InputActivityState::Idle
    };
    InputActivitySignal {
        state,
        health: SignalHealth::Ready,
    }
}

pub(super) fn classify_bundle_id(bundle_id: &str) -> ForegroundCategory {
    let normalized = bundle_id.to_ascii_lowercase();
    if [
        "1password",
        "keychain",
        "bank",
        "wallet",
        "authenticator",
        "password",
    ]
    .iter()
    .any(|part| normalized.contains(part))
    {
        ForegroundCategory::Sensitive
    } else if [
        "zoom", "teams", "slack", "discord", "facetime", "webex", "meet",
    ]
    .iter()
    .any(|part| normalized.contains(part))
    {
        ForegroundCategory::Communication
    } else if [
        "xcode",
        "visual-studio-code",
        "vscode",
        "jetbrains",
        "terminal",
        "iterm",
        "warp",
        "zed",
        "cursor",
    ]
    .iter()
    .any(|part| normalized.contains(part))
    {
        ForegroundCategory::Coding
    } else if [
        "pages", "word", "notion", "obsidian", "textedit", "bear", "ulysses",
    ]
    .iter()
    .any(|part| normalized.contains(part))
    {
        ForegroundCategory::Writing
    } else if ["safari", "chrome", "firefox", "arc", "edge", "brave"]
        .iter()
        .any(|part| normalized.contains(part))
    {
        ForegroundCategory::Browser
    } else if ["spotify", "music", "vlc", "quicktime", "netflix", "youtube"]
        .iter()
        .any(|part| normalized.contains(part))
    {
        ForegroundCategory::Media
    } else if normalized.is_empty() {
        ForegroundCategory::Unknown
    } else {
        ForegroundCategory::Other
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn raw_bundle_identifiers_are_projected_to_bounded_categories() {
        assert_eq!(
            classify_bundle_id("com.microsoft.VSCode"),
            ForegroundCategory::Coding
        );
        assert_eq!(
            classify_bundle_id("us.zoom.xos"),
            ForegroundCategory::Communication
        );
        assert_eq!(
            classify_bundle_id("com.agilebits.onepassword7"),
            ForegroundCategory::Sensitive
        );
        assert_eq!(
            classify_bundle_id("com.example.private-name"),
            ForegroundCategory::Other
        );
    }

    #[test]
    fn input_activity_boundaries_and_invalid_values_are_fixed() {
        use super::super::contracts::InputActivityState;

        let parameters = CalibrationParameters::default();
        for seconds in [0.0, 30.0, 30.0009] {
            assert_eq!(
                classify_input_activity_seconds(seconds, &parameters).state,
                InputActivityState::Active
            );
        }
        for seconds in [30.001, 300.0] {
            assert_eq!(
                classify_input_activity_seconds(seconds, &parameters).state,
                InputActivityState::Recent
            );
        }
        assert_eq!(
            classify_input_activity_seconds(300.001, &parameters).state,
            InputActivityState::Idle
        );
        for seconds in [f64::NAN, f64::INFINITY, -1.0, f64::MAX] {
            assert_eq!(
                classify_input_activity_seconds(seconds, &parameters),
                InputActivitySignal {
                    state: InputActivityState::Unknown,
                    health: SignalHealth::Degraded,
                }
            );
        }
    }
}
