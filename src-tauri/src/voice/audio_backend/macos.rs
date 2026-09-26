#![allow(non_camel_case_types)]
use super::{
    converter::{i16_to_f32_mono, CaptureDownsampler, LinearResampler, VPIO_RATE},
    ring::SpscF32,
    AudioBackendStatus, CaptureSink, DuckingLevel, VoiceProcessingConfig, AIRPLAY_TRANSPORT,
    BLUETOOTH_TRANSPORT,
};
use crate::RunCancellation;
use std::{
    ffi::{c_char, c_void, CStr},
    path::Path,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    thread,
    time::Duration,
};

const FRAME_SAMPLES: usize = 160;
const TRANSPORT_BLUETOOTH: u32 = BLUETOOTH_TRANSPORT;
const TRANSPORT_AIRPLAY: u32 = AIRPLAY_TRANSPORT;

#[repr(C)]
struct SaaaVpioConfig {
    sample_rate: u32,
    ducking_level: u32,
    enable_advanced_ducking: u8,
    enable_agc: u8,
    bypass_voice_processing: u8,
    reserved: u8,
}

#[repr(C)]
struct SaaaVpioStatus {
    sample_rate: u32,
    ducking_level: u32,
    output_transport: u32,
    advanced_ducking: u8,
    agc_enabled: u8,
    bypass_enabled: u8,
    macos_major: u8,
}

type RingWrite = unsafe extern "C" fn(*mut c_void, *const f32, u32) -> u32;
type RingRead = unsafe extern "C" fn(*mut c_void, *mut f32, u32) -> u32;
type FlagFn = unsafe extern "C" fn(*mut c_void);

extern "C" {
    fn saaa_vpio_create(
        config: *const SaaaVpioConfig,
        playback_ring: *mut c_void,
        capture_ring: *mut c_void,
        write_capture: RingWrite,
        read_playback: RingRead,
        on_route_change: Option<FlagFn>,
        route_ctx: *mut c_void,
        err: *mut c_char,
        err_len: u32,
    ) -> *mut c_void;
    fn saaa_vpio_start(session: *mut c_void, err: *mut c_char, err_len: u32) -> i32;
    fn saaa_vpio_stop(session: *mut c_void) -> i32;
    fn saaa_vpio_readback(session: *mut c_void, status: *mut SaaaVpioStatus) -> i32;
    fn saaa_vpio_rebuild_requested(session: *const c_void) -> i32;
    fn saaa_vpio_capture_failed(session: *const c_void) -> i32;
    fn saaa_vpio_clear_rebuild(session: *mut c_void);
    fn saaa_vpio_destroy(session: *mut c_void);
    fn saaa_audio_default_output_transport(transport: *mut u32) -> i32;
    fn saaa_macos_major_version() -> u8;
}

unsafe extern "C" fn write_capture(ring: *mut c_void, src: *const f32, count: u32) -> u32 {
    super::ring::saaa_spsc_write_f32(ring.cast(), src, count)
}

unsafe extern "C" fn read_playback(ring: *mut c_void, dst: *mut f32, count: u32) -> u32 {
    super::ring::saaa_spsc_read_f32(ring.cast(), dst, count)
}

fn empty_status() -> SaaaVpioStatus {
    SaaaVpioStatus {
        sample_rate: 0,
        ducking_level: 0,
        output_transport: 0,
        advanced_ducking: 0,
        agc_enabled: 0,
        bypass_enabled: 0,
        macos_major: 0,
    }
}

fn c_error(buffer: &[c_char]) -> String {
    let bytes = unsafe { CStr::from_ptr(buffer.as_ptr()) }.to_bytes();
    String::from_utf8_lossy(bytes).into_owned()
}

fn transport_name(value: u32) -> Option<&'static str> {
    match value {
        TRANSPORT_BLUETOOTH => Some("bluetooth"),
        TRANSPORT_AIRPLAY => Some("airplay"),
        _ => Some("built-in"),
    }
}

pub fn macos_major() -> u8 {
    unsafe { saaa_macos_major_version() }
}

pub fn default_output_transport() -> Option<u32> {
    let mut transport = 0;
    let result = unsafe { saaa_audio_default_output_transport(&mut transport) };
    (result == 0).then_some(transport)
}

struct NativeUnit {
    ptr: *mut c_void,
}

unsafe impl Send for NativeUnit {}

impl NativeUnit {
    fn create(
        config: VoiceProcessingConfig,
        playback: &Arc<SpscF32>,
        capture: &Arc<SpscF32>,
    ) -> Result<(Self, SaaaVpioStatus), String> {
        let ffi_config = SaaaVpioConfig {
            sample_rate: VPIO_RATE,
            ducking_level: config.ducking.as_vpio_level(),
            enable_advanced_ducking: 0,
            enable_agc: 0,
            bypass_voice_processing: if config.bypass { 1 } else { 0 },
            reserved: 0,
        };
        let mut err = [0 as c_char; 256];
        let ptr = unsafe {
            saaa_vpio_create(
                &ffi_config,
                Arc::as_ptr(playback) as *mut SpscF32 as *mut c_void,
                Arc::as_ptr(capture) as *mut SpscF32 as *mut c_void,
                write_capture,
                read_playback,
                None,
                std::ptr::null_mut(),
                err.as_mut_ptr(),
                err.len() as u32,
            )
        };
        if ptr.is_null() {
            return Err(c_error(&err));
        }
        if unsafe { saaa_vpio_start(ptr, err.as_mut_ptr(), err.len() as u32) } != 0 {
            unsafe { saaa_vpio_destroy(ptr) };
            return Err(c_error(&err));
        }
        let mut status = empty_status();
        if unsafe { saaa_vpio_readback(ptr, &mut status) } != 0 {
            unsafe { saaa_vpio_destroy(ptr) };
            return Err("Could not read VoiceProcessing settings after start".into());
        }
        Ok((Self { ptr }, status))
    }

    fn rebuild_requested(&self) -> bool {
        unsafe { saaa_vpio_rebuild_requested(self.ptr) != 0 }
    }

    fn capture_failed(&self) -> bool {
        unsafe { saaa_vpio_capture_failed(self.ptr) != 0 }
    }

    fn clear_rebuild(&self) {
        unsafe { saaa_vpio_clear_rebuild(self.ptr) };
    }
}

impl Drop for NativeUnit {
    fn drop(&mut self) {
        if !self.ptr.is_null() {
            unsafe {
                saaa_vpio_stop(self.ptr);
                saaa_vpio_destroy(self.ptr);
            }
            self.ptr = std::ptr::null_mut();
        }
    }
}

pub struct MacEngine {
    playback: Arc<SpscF32>,
    capture: Arc<SpscF32>,
    unit: Mutex<Option<NativeUnit>>,
    stop: Arc<AtomicBool>,
    worker: Mutex<Option<thread::JoinHandle<()>>>,
    sink: Mutex<Option<Arc<CaptureSink>>>,
    config: Mutex<VoiceProcessingConfig>,
    last_status: Mutex<SaaaVpioStatus>,
    playback_active: AtomicBool,
    capture_active: AtomicBool,
    playback_resampler: Mutex<LinearResampler>,
}

impl MacEngine {
    pub fn new() -> Self {
        Self {
            playback: Arc::new(SpscF32::new()),
            capture: Arc::new(SpscF32::new()),
            unit: Mutex::new(None),
            stop: Arc::new(AtomicBool::new(false)),
            worker: Mutex::new(None),
            sink: Mutex::new(None),
            config: Mutex::new(VoiceProcessingConfig::default()),
            last_status: Mutex::new(SaaaVpioStatus {
                sample_rate: VPIO_RATE,
                ducking_level: DuckingLevel::Min.as_vpio_level(),
                output_transport: 0,
                advanced_ducking: 0,
                agc_enabled: 0,
                bypass_enabled: 0,
                macos_major: macos_major(),
            }),
            playback_active: AtomicBool::new(false),
            capture_active: AtomicBool::new(false),
            playback_resampler: Mutex::new(LinearResampler::new(24_000, VPIO_RATE)),
        }
    }

    pub fn status(&self) -> AudioBackendStatus {
        let last = self.last_status.lock().unwrap_or_else(|e| e.into_inner());
        let available = macos_major() >= 14;
        let transport = default_output_transport().unwrap_or(last.output_transport);
        AudioBackendStatus {
            available,
            reason: if available {
                None
            } else {
                Some("VoiceProcessing ducking requires macOS 14 or later".into())
            },
            capture_active: self.capture_active.load(Ordering::Acquire),
            playback_active: self.playback_active.load(Ordering::Acquire),
            aec_active: self.capture_active.load(Ordering::Acquire) && last.bypass_enabled == 0,
            ducking_level: match last.ducking_level {
                0 => "default",
                20 => "mid",
                30 => "max",
                _ => "min",
            }
            .into(),
            agc_enabled: last.agc_enabled != 0,
            output_transport: transport_name(transport).map(str::to_string),
            macos_major: last.macos_major,
        }
    }

    pub fn start_capture(
        self: &Arc<Self>,
        config: VoiceProcessingConfig,
        sink: Arc<CaptureSink>,
    ) -> Result<AudioBackendStatus, String> {
        if macos_major() < 14 {
            return Err("VoiceProcessing ducking requires macOS 14 or later".into());
        }
        let transport = default_output_transport().unwrap_or(0);
        if transport == TRANSPORT_AIRPLAY {
            return Err("AirPlay output does not use VoiceProcessing AEC".into());
        }
        if transport == TRANSPORT_BLUETOOTH && !config.vpio_on_bluetooth {
            return Err("Bluetooth output skips VoiceProcessing by default".into());
        }
        self.stop_capture();
        *self.config.lock().unwrap_or_else(|e| e.into_inner()) = config;
        *self.sink.lock().unwrap_or_else(|e| e.into_inner()) = Some(sink);
        if let Err(error) = self.open_unit(config) {
            *self.sink.lock().unwrap_or_else(|e| e.into_inner()) = None;
            return Err(error);
        }
        self.capture_active.store(true, Ordering::Release);
        self.stop.store(false, Ordering::Release);
        let capture = self.capture.clone();
        let stop = self.stop.clone();
        let sink = self.sink.lock().unwrap_or_else(|e| e.into_inner()).clone();
        let engine = Arc::clone(self);
        let handle = match thread::Builder::new()
            .name("saaa-vpio-capture".into())
            .spawn(move || capture_worker(engine, capture, stop, sink))
        {
            Ok(handle) => handle,
            Err(_) => {
                self.capture_active.store(false, Ordering::Release);
                *self.sink.lock().unwrap_or_else(|e| e.into_inner()) = None;
                *self.unit.lock().unwrap_or_else(|e| e.into_inner()) = None;
                return Err("Could not start VoiceProcessing capture worker".into());
            }
        };
        *self.worker.lock().unwrap_or_else(|e| e.into_inner()) = Some(handle);
        Ok(self.status())
    }

    fn open_unit(&self, config: VoiceProcessingConfig) -> Result<(), String> {
        let (unit, status) = NativeUnit::create(config, &self.playback, &self.capture)?;
        if status.agc_enabled != 0 {
            drop(unit);
            return Err("VoiceProcessing AGC stayed enabled after initialize".into());
        }
        if status.bypass_enabled != u8::from(config.bypass)
            || status.advanced_ducking != 0
            || status.ducking_level != config.ducking.as_vpio_level()
        {
            drop(unit);
            return Err("VoiceProcessing settings changed after initialize".into());
        }
        *self.last_status.lock().unwrap_or_else(|e| e.into_inner()) = status;
        *self.unit.lock().unwrap_or_else(|e| e.into_inner()) = Some(unit);
        Ok(())
    }

    /// Returns whether the capture worker should keep running.
    /// Must not join the capture worker (it may be the caller).
    pub fn rebuild_if_needed(&self) -> bool {
        let should = self
            .unit
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
            .is_some_and(NativeUnit::rebuild_requested);
        if !should || !self.capture_active.load(Ordering::Acquire) {
            return true;
        }
        let config = *self.config.lock().unwrap_or_else(|e| e.into_inner());
        let transport = default_output_transport().unwrap_or(0);
        if transport == TRANSPORT_AIRPLAY
            || (transport == TRANSPORT_BLUETOOTH && !config.vpio_on_bluetooth)
        {
            *self.unit.lock().unwrap_or_else(|e| e.into_inner()) = None;
            self.capture_active.store(false, Ordering::Release);
            self.playback_active.store(false, Ordering::Release);
            return false;
        }
        *self.unit.lock().unwrap_or_else(|e| e.into_inner()) = None;
        if let Ok(()) = self.open_unit(config) {
            if let Some(unit) = self.unit.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
                unit.clear_rebuild();
            }
            return true;
        }
        self.capture_active.store(false, Ordering::Release);
        false
    }

    pub fn unit_rebuild_requested(&self) -> bool {
        self.unit
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
            .is_some_and(NativeUnit::rebuild_requested)
    }

    pub fn unit_capture_failed(&self) -> bool {
        self.unit
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
            .is_some_and(NativeUnit::capture_failed)
    }

    pub fn stop_capture(&self) {
        self.stop.store(true, Ordering::Release);
        self.capture_active.store(false, Ordering::Release);
        if let Some(handle) = self.worker.lock().unwrap_or_else(|e| e.into_inner()).take() {
            let _ = handle.join();
        }
        *self.sink.lock().unwrap_or_else(|e| e.into_inner()) = None;
        *self.unit.lock().unwrap_or_else(|e| e.into_inner()) = None;
        self.capture.clear();
        self.interrupt_playback();
    }

    pub fn is_capturing(&self) -> bool {
        self.capture_active.load(Ordering::Acquire)
    }

    pub fn queue_i16(&self, rate: u32, channels: u16, samples: &[i16]) -> bool {
        if !self.capture_active.load(Ordering::Acquire) {
            return false;
        }
        let generation = self.playback.generation();
        let mut mono = Vec::new();
        i16_to_f32_mono(samples, channels, &mut mono);
        let mut up = Vec::new();
        {
            let mut resampler = self
                .playback_resampler
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            if resampler_from(&resampler) != rate.max(1) {
                resampler.reset(rate.max(1));
            }
            resampler.push(&mono, &mut up);
        }
        if self.playback.generation() != generation {
            return true;
        }
        self.playback_active.store(true, Ordering::Release);
        let mut offset = 0;
        while offset < up.len() {
            let wrote = {
                let _guard = super::lock_playback();
                if self.playback.generation() != generation {
                    return true;
                }
                self.playback.write(&up[offset..])
            };
            if wrote == 0 {
                thread::sleep(Duration::from_millis(2));
                if !self.capture_active.load(Ordering::Acquire) {
                    return true;
                }
                continue;
            }
            offset += wrote;
        }
        true
    }

    pub fn interrupt_playback(&self) {
        let _guard = super::lock_playback();
        self.playback.clear();
        self.playback_active.store(false, Ordering::Release);
    }

    pub fn wait_playback_drained(&self, cancellation: &RunCancellation) -> Result<(), String> {
        while self.playback.len() > 0 {
            if cancellation.is_cancelled() {
                self.interrupt_playback();
                return Err("Speech cancelled".into());
            }
            if !self.capture_active.load(Ordering::Acquire) {
                self.interrupt_playback();
                return Ok(());
            }
            thread::sleep(Duration::from_millis(5));
        }
        self.playback_active.store(false, Ordering::Release);
        Ok(())
    }

    pub fn play_wav_blocking(
        &self,
        path: &Path,
        cancellation: &RunCancellation,
        on_started: &mut Option<Box<dyn FnOnce() + Send>>,
    ) -> Result<bool, String> {
        if !self.capture_active.load(Ordering::Acquire) {
            return Ok(false);
        }
        let bytes =
            std::fs::read(path).map_err(|_| "Could not read the TTS artifact".to_string())?;
        let mut decoder = crate::voice::http_audio::decode::Decoder::new("wav")?;
        let samples = decoder.push(&bytes)?;
        decoder.finish()?;
        let format = decoder
            .format
            .ok_or_else(|| "TTS artifact is missing a PCM format".to_string())?;
        if let Some(callback) = on_started.take() {
            callback();
        }
        if !self.queue_i16(format.rate, format.channels, &samples) {
            return Ok(false);
        }
        self.wait_playback_drained(cancellation)?;
        Ok(true)
    }
}

fn resampler_from(resampler: &LinearResampler) -> u32 {
    resampler.from_rate()
}

fn capture_worker(
    engine: Arc<MacEngine>,
    capture: Arc<SpscF32>,
    stop: Arc<AtomicBool>,
    sink: Option<Arc<CaptureSink>>,
) {
    let mut downsampler = CaptureDownsampler::new();
    let mut pending = Vec::new();
    let mut scratch = vec![0.0; 2048];
    let Some(sink) = sink else { return };
    'capture: while !stop.load(Ordering::Acquire) {
        if engine.unit_capture_failed() {
            break;
        }
        if engine.unit_rebuild_requested() && !engine.rebuild_if_needed() {
            break;
        }
        let n = capture.read(&mut scratch);
        if n == 0 {
            thread::sleep(Duration::from_millis(2));
            continue;
        }
        downsampler.push(&scratch[..n], &mut pending);
        while pending.len() >= FRAME_SAMPLES {
            let frame = pending.drain(..FRAME_SAMPLES).collect::<Vec<_>>();
            if !sink(frame) {
                break 'capture;
            }
        }
    }
    engine.capture_active.store(false, Ordering::Release);
    engine.playback_active.store(false, Ordering::Release);
    *engine.unit.lock().unwrap_or_else(|e| e.into_inner()) = None;
}

#[cfg(test)]
mod tests {
    #[test]
    fn vpio_c_source_keeps_callbacks_allocation_free() {
        let source = include_str!("../../../native/macos_vpio.c");
        let render = source
            .split("static OSStatus render_cb")
            .nth(1)
            .and_then(|rest| rest.split("static OSStatus input_cb").next())
            .unwrap_or("");
        let input = source
            .split("static OSStatus input_cb")
            .nth(1)
            .and_then(|rest| rest.split("static OSStatus route_listener").next())
            .unwrap_or("");
        for body in [render, input] {
            assert!(!body.contains("malloc"));
            assert!(!body.contains("calloc"));
            assert!(!body.contains("realloc"));
            assert!(!body.contains("NSLock"));
            assert!(!body.contains("dispatch_async"));
        }
    }
}
