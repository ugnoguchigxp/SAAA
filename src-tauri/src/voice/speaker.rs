use libloading::Library;
use std::{
    ffi::{c_char, c_float, c_int, CString},
    path::Path,
    ptr,
    sync::{mpsc, Arc},
    thread,
    time::Duration,
};
use zeroize::Zeroize;

const SAMPLE_RATE: i32 = 16_000;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

#[repr(C)]
struct SherpaOnnxSpeakerEmbeddingExtractorConfig {
    model: *const c_char,
    num_threads: c_int,
    debug: c_int,
    provider: *const c_char,
}

#[repr(C)]
struct SherpaOnnxSpeakerEmbeddingExtractor {
    _private: [u8; 0],
}

#[repr(C)]
struct SherpaOnnxOnlineStream {
    _private: [u8; 0],
}

type CreateExtractor = unsafe extern "C" fn(
    *const SherpaOnnxSpeakerEmbeddingExtractorConfig,
) -> *const SherpaOnnxSpeakerEmbeddingExtractor;
type DestroyExtractor = unsafe extern "C" fn(*const SherpaOnnxSpeakerEmbeddingExtractor);
type ExtractorDim = unsafe extern "C" fn(*const SherpaOnnxSpeakerEmbeddingExtractor) -> c_int;
type CreateStream = unsafe extern "C" fn(
    *const SherpaOnnxSpeakerEmbeddingExtractor,
) -> *const SherpaOnnxOnlineStream;
type AcceptWaveform =
    unsafe extern "C" fn(*const SherpaOnnxOnlineStream, c_int, *const c_float, c_int);
type InputFinished = unsafe extern "C" fn(*const SherpaOnnxOnlineStream);
type IsReady = unsafe extern "C" fn(
    *const SherpaOnnxSpeakerEmbeddingExtractor,
    *const SherpaOnnxOnlineStream,
) -> c_int;
type ComputeEmbedding = unsafe extern "C" fn(
    *const SherpaOnnxSpeakerEmbeddingExtractor,
    *const SherpaOnnxOnlineStream,
) -> *const c_float;
type DestroyEmbedding = unsafe extern "C" fn(*const c_float);
type DestroyStream = unsafe extern "C" fn(*const SherpaOnnxOnlineStream);

enum Request {
    Embed {
        samples: Vec<f32>,
        response: mpsc::SyncSender<Result<Vec<f32>, String>>,
    },
    Shutdown,
}

struct SpeakerExtractorInner {
    sender: mpsc::Sender<Request>,
    dimension: usize,
}

impl Drop for SpeakerExtractorInner {
    fn drop(&mut self) {
        let _ = self.sender.send(Request::Shutdown);
    }
}

/// Thread-safe handle for the native extractor. The native library and all of
/// its opaque handles remain owned by the worker thread for their full lifetime.
#[derive(Clone)]
pub struct SpeakerExtractor {
    inner: Arc<SpeakerExtractorInner>,
}

impl SpeakerExtractor {
    pub fn start(library_path: &Path, model_path: &Path) -> Result<Self, String> {
        if !library_path.is_file() {
            return Err("The bundled speaker-verification library is missing".to_string());
        }
        if !model_path.is_file() {
            return Err("The bundled speaker-verification model is missing".to_string());
        }
        let library_path = library_path.to_path_buf();
        let model_path = model_path.to_path_buf();
        let (sender, receiver) = mpsc::channel();
        let (ready_sender, ready_receiver) = mpsc::sync_channel(1);
        thread::Builder::new()
            .name("speaker-embedding".to_string())
            .spawn(move || {
                let mut native = match NativeExtractor::load(&library_path, &model_path) {
                    Ok(native) => native,
                    Err(error) => {
                        let _ = ready_sender.send(Err(error));
                        return;
                    }
                };
                let _ = ready_sender.send(Ok(native.dimension));
                while let Ok(request) = receiver.recv() {
                    match request {
                        Request::Embed {
                            mut samples,
                            response,
                        } => {
                            let result = native.embed(&samples);
                            samples.zeroize();
                            let _ = response.send(result);
                        }
                        Request::Shutdown => break,
                    }
                }
            })
            .map_err(|error| format!("Could not start the speaker-verification worker: {error}"))?;
        let dimension = ready_receiver
            .recv_timeout(REQUEST_TIMEOUT)
            .map_err(|_| "Speaker-verification initialization timed out".to_string())??;
        Ok(Self {
            inner: Arc::new(SpeakerExtractorInner { sender, dimension }),
        })
    }

    pub fn dimension(&self) -> usize {
        self.inner.dimension
    }

    pub fn embed(&self, samples: Vec<f32>) -> Result<Vec<f32>, String> {
        if samples.len() < SAMPLE_RATE as usize || samples.len() > SAMPLE_RATE as usize * 30 {
            return Err(
                "Speaker verification requires between one and thirty seconds of audio".to_string(),
            );
        }
        if samples.iter().any(|sample| !sample.is_finite()) {
            return Err("Speaker verification received invalid audio".to_string());
        }
        let (response_sender, response_receiver) = mpsc::sync_channel(1);
        self.inner
            .sender
            .send(Request::Embed {
                samples,
                response: response_sender,
            })
            .map_err(|_| "Speaker-verification worker is unavailable".to_string())?;
        response_receiver
            .recv_timeout(REQUEST_TIMEOUT)
            .map_err(|_| "Speaker-verification inference timed out".to_string())?
    }
}

struct NativeExtractor {
    _library: Library,
    extractor: *const SherpaOnnxSpeakerEmbeddingExtractor,
    dimension: usize,
    destroy_extractor: DestroyExtractor,
    create_stream: CreateStream,
    accept_waveform: AcceptWaveform,
    input_finished: InputFinished,
    is_ready: IsReady,
    compute_embedding: ComputeEmbedding,
    destroy_embedding: DestroyEmbedding,
    destroy_stream: DestroyStream,
}

impl NativeExtractor {
    fn load(library_path: &Path, model_path: &Path) -> Result<Self, String> {
        let library = unsafe { Library::new(library_path) }
            .map_err(|error| format!("Could not load the speaker-verification library: {error}"))?;
        let create_extractor: CreateExtractor =
            load_symbol(&library, b"SherpaOnnxCreateSpeakerEmbeddingExtractor\0")?;
        let destroy_extractor: DestroyExtractor =
            load_symbol(&library, b"SherpaOnnxDestroySpeakerEmbeddingExtractor\0")?;
        let extractor_dim: ExtractorDim =
            load_symbol(&library, b"SherpaOnnxSpeakerEmbeddingExtractorDim\0")?;
        let create_stream: CreateStream = load_symbol(
            &library,
            b"SherpaOnnxSpeakerEmbeddingExtractorCreateStream\0",
        )?;
        let accept_waveform: AcceptWaveform =
            load_symbol(&library, b"SherpaOnnxOnlineStreamAcceptWaveform\0")?;
        let input_finished: InputFinished =
            load_symbol(&library, b"SherpaOnnxOnlineStreamInputFinished\0")?;
        let is_ready: IsReady =
            load_symbol(&library, b"SherpaOnnxSpeakerEmbeddingExtractorIsReady\0")?;
        let compute_embedding: ComputeEmbedding = load_symbol(
            &library,
            b"SherpaOnnxSpeakerEmbeddingExtractorComputeEmbedding\0",
        )?;
        let destroy_embedding: DestroyEmbedding = load_symbol(
            &library,
            b"SherpaOnnxSpeakerEmbeddingExtractorDestroyEmbedding\0",
        )?;
        let destroy_stream: DestroyStream =
            load_symbol(&library, b"SherpaOnnxDestroyOnlineStream\0")?;

        let model = CString::new(model_path.to_string_lossy().as_bytes())
            .map_err(|_| "Speaker model path contains an invalid null byte".to_string())?;
        let provider = CString::new("cpu").expect("static provider is valid");
        let config = SherpaOnnxSpeakerEmbeddingExtractorConfig {
            model: model.as_ptr(),
            num_threads: 2,
            debug: 0,
            provider: provider.as_ptr(),
        };
        let extractor = unsafe { create_extractor(&config) };
        if extractor.is_null() {
            return Err("Could not initialize the bundled speaker-verification model".to_string());
        }
        let dimension = unsafe { extractor_dim(extractor) };
        if dimension <= 0 || dimension > 4_096 {
            unsafe { destroy_extractor(extractor) };
            return Err(
                "Speaker-verification model returned an invalid embedding dimension".to_string(),
            );
        }
        Ok(Self {
            _library: library,
            extractor,
            dimension: dimension as usize,
            destroy_extractor,
            create_stream,
            accept_waveform,
            input_finished,
            is_ready,
            compute_embedding,
            destroy_embedding,
            destroy_stream,
        })
    }

    fn embed(&mut self, samples: &[f32]) -> Result<Vec<f32>, String> {
        let sample_count = i32::try_from(samples.len())
            .map_err(|_| "Speaker-verification audio is too long".to_string())?;
        let stream = unsafe { (self.create_stream)(self.extractor) };
        if stream.is_null() {
            return Err("Could not create a speaker-verification stream".to_string());
        }
        unsafe {
            (self.accept_waveform)(stream, SAMPLE_RATE, samples.as_ptr(), sample_count);
            (self.input_finished)(stream);
        }
        if unsafe { (self.is_ready)(self.extractor, stream) } == 0 {
            unsafe { (self.destroy_stream)(stream) };
            return Err("The recording does not contain enough usable speech".to_string());
        }
        let embedding = unsafe { (self.compute_embedding)(self.extractor, stream) };
        if embedding.is_null() {
            unsafe { (self.destroy_stream)(stream) };
            return Err("Speaker-verification inference failed".to_string());
        }
        let result = unsafe { std::slice::from_raw_parts(embedding, self.dimension) }.to_vec();
        unsafe {
            (self.destroy_embedding)(embedding);
            (self.destroy_stream)(stream);
        }
        if result.iter().any(|value| !value.is_finite()) {
            return Err("Speaker-verification model returned an invalid embedding".to_string());
        }
        Ok(result)
    }
}

impl Drop for NativeExtractor {
    fn drop(&mut self) {
        if !self.extractor.is_null() {
            unsafe { (self.destroy_extractor)(self.extractor) };
            self.extractor = ptr::null();
        }
    }
}

fn load_symbol<T: Copy>(library: &Library, name: &[u8]) -> Result<T, String> {
    unsafe { library.get::<T>(name) }
        .map(|symbol| *symbol)
        .map_err(|error| format!("Bundled speaker library is incompatible: {error}"))
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;

    #[test]
    fn bundled_runtime_loads_and_extracts_a_finite_embedding() {
        let resource = Path::new(env!("CARGO_MANIFEST_DIR")).join("resources/voice");
        let extractor = SpeakerExtractor::start(
            &resource.join("lib/libsherpa-onnx-c-api.dylib"),
            &resource.join("model/3dspeaker_speech_campplus_sv_zh-cn_16k-common.onnx"),
        )
        .expect("bundled runtime loads");
        let samples = (0..SAMPLE_RATE * 3)
            .map(|index| {
                ((index as f32 / SAMPLE_RATE as f32) * 220.0 * std::f32::consts::TAU).sin() * 0.1
            })
            .collect();
        let embedding = extractor.embed(samples).expect("embedding extracts");
        assert_eq!(embedding.len(), extractor.dimension());
        assert!(embedding.iter().all(|value| value.is_finite()));
    }
}
