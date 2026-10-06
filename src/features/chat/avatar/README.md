# Light avatar background

The procedural B spirit from the 2026-10-06 avatar handoff is bundled locally with
Three.js 0.186.1. The renderer uses no reference image, iframe, demo UI or CDN;
speech playback remains owned by the backend. The original 12 eight-second motion phrases and 0.7 second pose
blend are retained. Only B is initialized; particle counts and tessellation are
reduced for the background (12,960 points, 61,542 triangles, 113 draw calls in the
Chromium preview). Pixel ratio is capped at 1.25; animation is capped at 24 fps.

`LightAvatarBackground` owns a transparent WebGL canvas. The separate chat tint
is `rgb(13 17 24 / 40%)`; message surfaces use alpha colors without fading text or
controls. The model stays outside the history scroll container.

Laya runs in the backend for **each exact dictionary-applied TTS text chunk**,
including streamed answer chunks, progress speech and replay. One native
`model`, `state`, `questions` request carries `state.utterance` and asks for both
`motion` (12 gestures plus neutral) and `voice` (natural, bright, gentle, serious,
excited). No SystemContext, conversation-history classification or token-level
frontend request is used. The unit-test page offers Laya basic choices and
Laya speech expression choices separately.

The same decision changes VOICEVOX speed, pitch and intonation relative to the
saved settings, using the existing bounded speech-expression presets. The
selected voice/style and persisted settings remain the base. Other synthesis
models keep their existing parameters. VOICEVOX's query controls are documented
in the [official guide](https://github.com/VOICEVOX/voicevox_core/blob/main/docs/guide/user/usage.md).

`useAvatarDecision` listens to `conversation-speech-expression` events. Each
chunk has a monotonically increasing ID. HTTP speech starts the pose when its
first decoded PCM packet enters the existing continuous player; prior chunks
have drained. Completion, errors and cancellation emit an end event and return
the pose to neutral. This is queue-time alignment, not hardware-sample-accurate
lip sync. System speech emits after synthesis, before WAV enqueue. Foreign,
malformed, duplicate and stale events cannot override the current pose.

Laya waits at most two seconds per chunk. Failure/timeout uses neutral/natural;
late results never reach playback. An in-flight decision finishes LARM cleanup
in the background, holding a slot so later chunks cannot build a request backlog.
Only Laya is claimed for inference; microphone capture, VPIO, AEC and the actual
rendered PCM reference keep their existing path. No playback flag gates ASR.

Each chosen phrase plays once, blends back to rest, and stops requesting frames.
The hidden route/document disposes its renderer, observers, events and GPU
resources. Reduced-motion keeps a static image. Late imports cannot install a
canvas after unmount. WebGL failure does not prevent using the chat.

Validation: frontend tests cover provider selection, chunk event ordering and
cancellation, finite animation, reduced-motion and resource release. Opt-in
Rust live tests are in `provider_unit_laya_live.rs` and
`crates/larm-session/tests/system_one_live.rs`; they use synthetic phrases and
read settings without editing the database. A browser preview verifies alpha,
input, repeated hide/show and absence of duplicate canvases.

Rust tests cover native request shape, voice parameter mapping, late/cancelled
inference, unchanged decoded PCM and one audio-ready notification per chunk.
The opt-in `laya_voicevox_synthesizes_expressive_chunks` test uses saved settings
read-only and generates real expressive VOICEVOX audio without playing it.
If no harness voice is saved, `SAAA_LAYA_TEST_VOICE` supplies a test-only voice;
it is never written back to settings.
Live microphone/speaker overlap acceptance is separate from these tests.
