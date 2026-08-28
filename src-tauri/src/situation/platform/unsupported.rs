use crate::situation::contracts::{
    unsupported_input_activity, CalibrationParameters, ForegroundCategory, ForegroundSignal,
    InputActivitySignal, SignalHealth,
};

pub fn foreground_signal() -> ForegroundSignal {
    ForegroundSignal {
        category: ForegroundCategory::Unknown,
        health: SignalHealth::Unsupported,
    }
}

pub fn input_activity_signal(_: &CalibrationParameters) -> InputActivitySignal {
    unsupported_input_activity()
}
