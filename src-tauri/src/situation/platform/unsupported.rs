use super::{ForegroundCategory, ForegroundSignal, SignalHealth};

pub fn foreground_signal() -> ForegroundSignal {
    ForegroundSignal {
        category: ForegroundCategory::Unknown,
        health: SignalHealth::Unsupported,
    }
}
