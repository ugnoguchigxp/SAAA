# Delegated work repair results

Date: 2026-09-21. HEAD at start: `f4ad46ddc36042fc419b175186ca7f5572741c24`.

## Commands run this session (delegated-work owned)

```
cargo test --manifest-path src-tauri/Cargo.toml --lib dw_r -- --test-threads=1
# 29 passed

cargo test --manifest-path src-tauri/Cargo.toml --lib steward:: -- --test-threads=1
# 66 passed, 2 ignored (opt-in TTS)

cargo test --manifest-path src-tauri/Cargo.toml --lib coding:: -- --test-threads=1
# 13 passed, 3 ignored (live SDK/Pi)

cargo test --manifest-path src-tauri/Cargo.toml --lib schedule::tests -- --test-threads=1
# 18 passed

bun test tests/delegated-work-cases.test.ts tests/delegated-work-cases-corpus.test.ts tests/delegated-work-panel.test.tsx tests/delegated-work-chat.test.tsx tests/steward-panel.test.tsx tests/coding-steward.test.ts tests/pi-codex-sdk.test.ts
# 11 passed

bun run ipc:check
# 6 passed

bun run delegated-work:eval
# scripted corpus ≥20
```

Real SDK/TTS remain opt-in ignored tests; they are not counted as pass.

## Other-work gates (not claimed as this theme)

`bun run size:check` still fails on role-routing/tool_selection/chat artifacts/window_size and similar files that this theme did not own. `check:local` was not claimed green. Steward-owned modules were registered without bulk-relaxing those other ratchets.

## Historical SDK/TTS (not re-run this session)

- 2026-09-21 prior results: authenticated SDK README read and write/network/scope reject; System TTS playback_finished and delivery_unknown. See dated rows in `results.md`.

## DWR-23

`tauri-plugin-wdio-webdriver` is not a production dependency. Adding it would instrument the app. XCUIAutomation is available (Xcode 26.3) but Accessibility permission was not granted in this session.

## DWR-24

External: live model auth, GUI session, audio output, sleep/wake. Not claimed as demonstrated.

## DWR-25

Fixture lane: `dw_r25_driver_and_drain_p95_under_two_seconds` (32 samples, budget 2000ms) and duplicate-wake test. Live app clocks and hold→message on a running GUI were not measured.

## Sample evidence row

`DWR-04 / dw_r04_proposal_creates_plan_task_reservation_and_intent_atomically / f4ad46d / source=user_message / grant=existing / tasks>=1 / intents=tasks / pass`
