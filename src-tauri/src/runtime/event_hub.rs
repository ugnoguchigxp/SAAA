//! UI event delivery shared by coding and capability runs.
use crate::ipc_contract::RuntimeEvent;
use std::time::Instant;

pub(crate) trait RuntimeEventSender: Send + Sync {
    fn send(&self, event: RuntimeEvent) -> tauri::Result<()>;
    fn send_received(&self, event: RuntimeEvent, _received_at: Instant) -> tauri::Result<()> {
        self.send(event)
    }
    fn clone_box(&self) -> Box<dyn RuntimeEventSender>;
}

impl RuntimeEventSender for tauri::ipc::Channel<RuntimeEvent> {
    fn send(&self, event: RuntimeEvent) -> tauri::Result<()> {
        tauri::ipc::Channel::send(self, event)
    }
    fn clone_box(&self) -> Box<dyn RuntimeEventSender> {
        Box::new(self.clone())
    }
}
