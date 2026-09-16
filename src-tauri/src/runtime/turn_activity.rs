use super::event_hub::RuntimeEventSender;
use crate::{ipc_contract::RuntimeEvent, memory::context_window::ContextHealthReport};

pub(crate) fn send_context_window_once(
    emitted: &mut bool,
    on_event: &dyn RuntimeEventSender,
    run_id: &str,
    health: &ContextHealthReport,
) {
    if *emitted {
        return;
    }
    let _ = on_event.send(RuntimeEvent::Activity {
        run_id: run_id.to_string(),
        kind: "context-window".to_string(),
        summary: format!(
            "Context {}: {}/{} input bytes, {} bytes output reserved, {} memory items, {} recent messages, {} continuity groups, {} loaded source messages omitted{}{}",
            health.status,
            health.projected_bytes,
            health.hard_limit_bytes,
            health.output_reserve_bytes,
            health.memory_item_count,
            health.recent_source_messages,
            health.continuity_group_count,
            health.omitted_loaded_source_messages,
            if health.source_history_truncated {
                ", older source history truncated"
            } else {
                ""
            },
            if health.repair_count > 0 {
                ", minimal reconstruction applied"
            } else {
                ""
            },
        ),
    });
    *emitted = true;
}
