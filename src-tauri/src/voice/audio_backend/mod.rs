pub mod commands;
mod config;
mod converter;
#[cfg(target_os = "macos")]
mod macos;
mod probe;
mod ring;

pub use config::{
    AudioBackendStatus, DuckingLevel, VoiceProcessingConfig, AIRPLAY_TRANSPORT, BLUETOOTH_TRANSPORT,
};
pub use probe::run as run_probe;

use crate::RunCancellation;
use std::{
    path::Path,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex, OnceLock,
    },
};

static BACKEND: OnceLock<Arc<AudioBackend>> = OnceLock::new();
pub(super) type CaptureSink = dyn Fn(Vec<f32>) -> bool + Send + Sync;

pub fn global() -> Arc<AudioBackend> {
    BACKEND
        .get_or_init(|| Arc::new(AudioBackend::default()))
        .clone()
}

pub fn aec_is_active() -> bool {
    global().status().aec_active
}

pub struct AudioBackend {
    #[cfg(target_os = "macos")]
    engine: Arc<macos::MacEngine>,
    #[cfg(not(target_os = "macos"))]
    _unused: (),
    running: AtomicBool,
}

impl Default for AudioBackend {
    fn default() -> Self {
        Self {
            #[cfg(target_os = "macos")]
            engine: Arc::new(macos::MacEngine::new()),
            #[cfg(not(target_os = "macos"))]
            _unused: (),
            running: AtomicBool::new(false),
        }
    }
}

impl AudioBackend {
    pub fn status(&self) -> AudioBackendStatus {
        #[cfg(target_os = "macos")]
        {
            return self.engine.status();
        }
        #[cfg(not(target_os = "macos"))]
        AudioBackendStatus {
            available: false,
            reason: Some("VoiceProcessingIO is only implemented on macOS".into()),
            capture_active: false,
            playback_active: false,
            aec_active: false,
            ducking_level: DuckingLevel::Min.as_str().into(),
            agc_enabled: false,
            output_transport: None,
            macos_major: 0,
        }
    }

    pub fn start_capture(
        &self,
        config: VoiceProcessingConfig,
        sink: Arc<CaptureSink>,
    ) -> Result<AudioBackendStatus, String> {
        #[cfg(target_os = "macos")]
        {
            let status = self.engine.start_capture(config, sink)?;
            self.running.store(true, Ordering::Release);
            return Ok(status);
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = (config, sink);
            Err("VoiceProcessingIO is only implemented on macOS".into())
        }
    }

    pub fn stop_capture(&self) {
        #[cfg(target_os = "macos")]
        self.engine.stop_capture();
        self.running.store(false, Ordering::Release);
    }

    pub fn queue_i16(&self, rate: u32, channels: u16, samples: &[i16]) -> bool {
        #[cfg(target_os = "macos")]
        {
            return self.engine.queue_i16(rate, channels, samples);
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = (rate, channels, samples);
            false
        }
    }

    pub fn interrupt_playback(&self) {
        #[cfg(target_os = "macos")]
        self.engine.interrupt_playback();
    }

    pub fn wait_playback_drained(&self, cancellation: &RunCancellation) -> Result<(), String> {
        #[cfg(target_os = "macos")]
        {
            return self.engine.wait_playback_drained(cancellation);
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = cancellation;
            Ok(())
        }
    }

    pub fn play_wav_blocking(
        &self,
        path: &Path,
        cancellation: &RunCancellation,
        on_started: &mut Option<Box<dyn FnOnce() + Send>>,
    ) -> Result<bool, String> {
        #[cfg(target_os = "macos")]
        {
            return self
                .engine
                .play_wav_blocking(path, cancellation, on_started);
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = (path, cancellation, on_started);
            Ok(false)
        }
    }

    pub fn is_capturing(&self) -> bool {
        #[cfg(target_os = "macos")]
        {
            return self.engine.is_capturing();
        }
        #[cfg(not(target_os = "macos"))]
        self.running.load(Ordering::Acquire)
    }
}

/// Serializes TTS enqueue with interrupt. Render callbacks never take this lock.
static PLAYBACK_LOCK: Mutex<()> = Mutex::new(());

pub(super) fn lock_playback() -> std::sync::MutexGuard<'static, ()> {
    PLAYBACK_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

pub fn queue_tts_packet(rate: u32, channels: u16, samples: &[i16]) -> bool {
    global().queue_i16(rate, channels, samples)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reports_unavailability_off_macos_or_a_status_on_macos() {
        let status = AudioBackend::default().status();
        if cfg!(target_os = "macos") {
            assert!(status.macos_major == 0 || status.macos_major >= 11);
        } else {
            assert!(!status.available);
        }
    }
}
