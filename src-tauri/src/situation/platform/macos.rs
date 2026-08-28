use super::{
    classify_bundle_id, classify_input_activity_seconds, CalibrationParameters, ForegroundSignal,
    InputActivitySignal, SignalHealth,
};
use objc2_app_kit::NSWorkspace;
use objc2_core_graphics::{CGEventSource, CGEventSourceStateID, CGEventType};

const ANY_INPUT_EVENT: CGEventType = CGEventType(u32::MAX);

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

pub fn input_activity_signal(parameters: &CalibrationParameters) -> InputActivitySignal {
    let seconds = CGEventSource::seconds_since_last_event_type(
        CGEventSourceStateID::CombinedSessionState,
        ANY_INPUT_EVENT,
    );
    classify_input_activity_seconds(seconds, parameters)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{ffi::c_int, thread, time::Duration};

    #[repr(C)]
    struct MachTimeValue {
        seconds: c_int,
        microseconds: c_int,
    }

    #[repr(C)]
    struct MachTaskBasicInfo {
        virtual_size: u64,
        resident_size: u64,
        resident_size_max: u64,
        user_time: MachTimeValue,
        system_time: MachTimeValue,
        policy: c_int,
        suspend_count: c_int,
    }

    unsafe extern "C" {
        #[link_name = "mach_task_self_"]
        static MACH_TASK_SELF: u32;
        #[link_name = "task_info"]
        fn read_task_info(task: u32, flavor: c_int, info: *mut c_int, count: *mut u32) -> c_int;
    }

    #[test]
    fn live_core_graphics_call_returns_only_a_bounded_category() {
        use crate::situation::contracts::InputActivityState;

        let signal = input_activity_signal(&CalibrationParameters::default());
        assert!(matches!(
            signal.state,
            InputActivityState::Active
                | InputActivityState::Recent
                | InputActivityState::Idle
                | InputActivityState::Unknown
        ));
    }

    #[test]
    #[ignore = "requires a 30 minute macOS sampling soak"]
    fn thirty_minute_input_activity_soak_has_bounded_memory() {
        let rss_before = resident_set_bytes().expect("starting RSS reads");
        let started = std::time::Instant::now();
        let mut samples = 0_u64;
        while started.elapsed() < Duration::from_secs(30 * 60) {
            let signal = input_activity_signal(&CalibrationParameters::default());
            assert_eq!(signal.health, SignalHealth::Ready);
            samples = samples.saturating_add(1);
            thread::sleep(Duration::from_secs(2));
        }
        let rss_after = resident_set_bytes().expect("ending RSS reads");
        assert!(samples >= 890, "sampling stopped early");
        assert!(
            rss_after <= rss_before.saturating_add(32 * 1_024 * 1_024),
            "RSS grew by more than 32 MiB"
        );
    }

    fn resident_set_bytes() -> Option<u64> {
        const MACH_TASK_BASIC_INFO: c_int = 20;
        let mut info = MachTaskBasicInfo {
            virtual_size: 0,
            resident_size: 0,
            resident_size_max: 0,
            user_time: MachTimeValue {
                seconds: 0,
                microseconds: 0,
            },
            system_time: MachTimeValue {
                seconds: 0,
                microseconds: 0,
            },
            policy: 0,
            suspend_count: 0,
        };
        let mut count =
            u32::try_from(std::mem::size_of::<MachTaskBasicInfo>() / std::mem::size_of::<u32>())
                .ok()?;
        // SAFETY: task_info receives a correctly sized, writable MachTaskBasicInfo buffer.
        let status = unsafe {
            read_task_info(
                MACH_TASK_SELF,
                MACH_TASK_BASIC_INFO,
                (&mut info as *mut MachTaskBasicInfo).cast::<c_int>(),
                &mut count,
            )
        };
        (status == 0).then_some(info.resident_size)
    }
}
