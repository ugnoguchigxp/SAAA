use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum WebSocketConnectionState {
    Connected,
    Connecting,
    Disconnected,
}
