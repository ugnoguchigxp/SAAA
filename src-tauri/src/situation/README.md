# Situation

Owner: sampled context signals, rule-based scene classification, hysteresis, calibration, and delivery/foreground policy inputs. A scene estimate is not user authorization.

Preserve:
- Raw signal, health/staleness, candidate scene, stable scene, and policy decision are distinct.
- Keep hysteresis and explicit-state precedence; do not let transient/noisy samples immediately override stable state.
- Missing or unhealthy signals are not evidence that the environment is idle or safe to interrupt.
- Speech holds affect delivery, not whether an answer exists. Do not mark held speech as successful playback or restart completed work.
- Calibration/shadow results must not silently replace the active policy or widen delegated authority.

Locate:
- Types/policy versions: `contracts.rs`; runtime state/commands: `mod.rs`, `mod.d/`.
- Platform acquisition: `platform/`, `monitor.rs`; sampled update: `tick.rs`.
- Classification/hysteresis: `classifier.rs` (`Hysteresis`); calibration: `calibration.rs`, `calibration.d/`.
- Speech hold: `speech.rs` (`speech_holds_tts`); consumers: `../steward/dispatch.rs`, `../steward/report.rs`, `../schedule/decide.rs`.
- Persisted events: `repository.rs`, `repository.d/`; World projection: `world_snapshot.rs`.
- Tests: `speech_tests.rs`, inline classifier/calibration/runtime tests.

Trace signal snapshot -> candidate -> stable revision -> consumer decision. Inspect foreground and speech policy separately; a classifier output alone does not prove a hold was applied.
