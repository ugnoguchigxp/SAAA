//! Policy-only scheduler decision. Runtime code supplies the local clock and idle duration so
//! tests and headless hosts do not depend on a particular timezone implementation.
use crate::role_routing::contracts::RoutingLearning;

pub(crate) fn should_start(
    settings: &RoutingLearning,
    local_minutes: u16,
    idle_seconds: u32,
    already_running: bool,
) -> bool {
    settings.enabled
        && !already_running
        && idle_seconds >= settings.idle_seconds
        && in_window(local_minutes, &settings.local_start, &settings.local_end)
}

fn in_window(now: u16, start: &str, end: &str) -> bool {
    let start = parse(start);
    let end = parse(end);
    if start == end {
        return true;
    }
    if start < end {
        now >= start && now < end
    } else {
        now >= start || now < end
    }
}
fn parse(value: &str) -> u16 {
    value[0..2].parse::<u16>().unwrap_or(0) * 60 + value[3..5].parse::<u16>().unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rr_32_scheduler_handles_overnight_window_and_idle_gate() {
        let mut learning = RoutingLearning::default();
        learning.enabled = true;
        learning.local_start = "23:00".into();
        learning.local_end = "02:00".into();
        learning.idle_seconds = 10;
        assert!(should_start(&learning, 30, 10, false));
        assert!(!should_start(&learning, 180, 10, false));
        assert!(!should_start(&learning, 30, 9, false));
        assert!(!should_start(&learning, 30, 10, true));
    }
}
