//! Pure eligibility rules for durable memory work. Execution remains opt-in and local.
// The runtime adapter remains intentionally staged until battery, thermal, and network
// policy signals are available; fail-closed policy tests still compile in production builds.
#![allow(dead_code)]

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IdleSignals {
    pub active_runtime_runs: usize,
    pub milliseconds_since_user_input: u64,
    pub meeting_active: bool,
    pub microphone_active: bool,
    pub battery_allowed: bool,
    pub thermal_allowed: bool,
    pub network_allowed: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdleBlocker {
    ForegroundRun,
    RecentUserInput,
    Meeting,
    Microphone,
    Battery,
    Thermal,
    Network,
}

pub const MINIMUM_IDLE_MILLISECONDS: u64 = 30_000;

pub fn eligibility(signals: IdleSignals) -> Result<(), IdleBlocker> {
    if signals.active_runtime_runs > 0 {
        return Err(IdleBlocker::ForegroundRun);
    }
    if signals.milliseconds_since_user_input < MINIMUM_IDLE_MILLISECONDS {
        return Err(IdleBlocker::RecentUserInput);
    }
    if signals.meeting_active {
        return Err(IdleBlocker::Meeting);
    }
    if signals.microphone_active {
        return Err(IdleBlocker::Microphone);
    }
    if !signals.battery_allowed {
        return Err(IdleBlocker::Battery);
    }
    if !signals.thermal_allowed {
        return Err(IdleBlocker::Thermal);
    }
    if !signals.network_allowed {
        return Err(IdleBlocker::Network);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn eligible() -> IdleSignals {
        IdleSignals {
            active_runtime_runs: 0,
            milliseconds_since_user_input: MINIMUM_IDLE_MILLISECONDS,
            meeting_active: false,
            microphone_active: false,
            battery_allowed: true,
            thermal_allowed: true,
            network_allowed: true,
        }
    }

    #[test]
    fn every_foreground_or_resource_signal_blocks_memory_work() {
        let mut signals = eligible();
        assert_eq!(eligibility(signals), Ok(()));

        signals.active_runtime_runs = 1;
        assert_eq!(eligibility(signals), Err(IdleBlocker::ForegroundRun));
        signals = eligible();
        signals.milliseconds_since_user_input -= 1;
        assert_eq!(eligibility(signals), Err(IdleBlocker::RecentUserInput));
        signals = eligible();
        signals.meeting_active = true;
        assert_eq!(eligibility(signals), Err(IdleBlocker::Meeting));
        signals = eligible();
        signals.microphone_active = true;
        assert_eq!(eligibility(signals), Err(IdleBlocker::Microphone));
        signals = eligible();
        signals.battery_allowed = false;
        assert_eq!(eligibility(signals), Err(IdleBlocker::Battery));
        signals = eligible();
        signals.thermal_allowed = false;
        assert_eq!(eligibility(signals), Err(IdleBlocker::Thermal));
        signals = eligible();
        signals.network_allowed = false;
        assert_eq!(eligibility(signals), Err(IdleBlocker::Network));
    }
}
