//! Policy-only scheduler decision. Runtime code supplies the local clock and idle duration so
//! tests and headless hosts do not depend on a particular timezone implementation.
use crate::role_routing::contracts::RoutingLearning;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StartReason {
    Window,
    MissedWindow,
}

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

/// Daily scheduler gate. `day_key` and `last_completed_day` are host-provided ISO local dates;
/// comparing them prevents a wall-clock rollback from starting a second run. A missed window may
/// run later the same local day once the app is idle, but never while foreground work is active.
pub(crate) fn should_start_daily(
    settings: &RoutingLearning,
    local_minutes: u16,
    idle_seconds: u32,
    already_running: bool,
    foreground_active: bool,
    day_key: &str,
    last_completed_day: Option<&str>,
) -> Option<StartReason> {
    if !settings.enabled
        || already_running
        || foreground_active
        || idle_seconds < settings.idle_seconds
        || last_completed_day.is_some_and(|last| last >= day_key)
    {
        return None;
    }
    if in_window(local_minutes, &settings.local_start, &settings.local_end) {
        return Some(StartReason::Window);
    }
    let end = parse(&settings.local_end);
    let missed = if parse(&settings.local_start) < end {
        local_minutes >= end
    } else {
        // For an overnight window, only the post-window morning is a missed opportunity.
        local_minutes >= end && local_minutes < parse(&settings.local_start)
    };
    missed.then_some(StartReason::MissedWindow)
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

    #[test]
    fn rr_34_missed_night_runs_once_and_clock_rollback_does_not_repeat() {
        let mut learning = RoutingLearning::default();
        learning.enabled = true;
        learning.local_start = "02:00".into();
        learning.local_end = "05:00".into();
        learning.idle_seconds = 10;
        assert_eq!(
            should_start_daily(&learning, 6 * 60, 10, false, false, "2026-09-21", None),
            Some(StartReason::MissedWindow)
        );
        assert_eq!(
            should_start_daily(
                &learning,
                3 * 60,
                10,
                false,
                false,
                "2026-09-21",
                Some("2026-09-21")
            ),
            None
        );
        assert_eq!(
            should_start_daily(
                &learning,
                3 * 60,
                10,
                false,
                false,
                "2026-09-20",
                Some("2026-09-21")
            ),
            None
        );
    }

    #[test]
    fn rr_34_foreground_preempts_before_the_next_page() {
        let mut learning = RoutingLearning::default();
        learning.enabled = true;
        learning.local_start = "00:00".into();
        learning.local_end = "23:59".into();
        learning.idle_seconds = 0;
        assert_eq!(
            should_start_daily(&learning, 60, 100, false, true, "2026-09-21", None),
            None
        );
    }
}
