use super::contracts::{
    CalendarSignal, CalendarState, ForegroundCategory, ForegroundSignal, SignalHealth, TimeBucket,
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
}
