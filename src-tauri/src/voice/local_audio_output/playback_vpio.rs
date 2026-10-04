use super::Packet;
use crate::RunCancellation;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::time::Duration;

pub(super) enum VpioExit {
    Completed,
    CaptureEnded(Option<Packet>, Vec<tokio::sync::oneshot::Sender<()>>),
}

#[path = "playback_vpio/worker.rs"]
mod worker;
pub(super) use worker::play_through_vpio;
