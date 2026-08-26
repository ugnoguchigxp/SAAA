use super::{classify_bundle_id, ForegroundSignal, SignalHealth};
use objc2_app_kit::NSWorkspace;

pub fn foreground_signal() -> ForegroundSignal {
    let workspace = NSWorkspace::sharedWorkspace();
    let Some(application) = workspace.frontmostApplication() else {
        return ForegroundSignal {
            category: super::ForegroundCategory::Unknown,
            health: SignalHealth::Degraded,
        };
    };
    let Some(bundle_id) = application.bundleIdentifier() else {
        return ForegroundSignal {
            category: super::ForegroundCategory::Unknown,
            health: SignalHealth::Degraded,
        };
    };
    ForegroundSignal {
        category: classify_bundle_id(&bundle_id.to_string()),
        health: SignalHealth::Ready,
    }
}
