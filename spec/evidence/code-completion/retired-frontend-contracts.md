# Retired frontend architecture contracts

The canonical Page describes a single Ornith conversation queue with continuously available ASR. These files invoked frontend hooks removed before this remediation; retaining them would require reviving the retired runtime. Behavioral coverage is migrated to `conversation-asr-continuous.test.ts`, `browser-voice-capture.test.ts`, `native-voice-capture-monitor.test.ts`, `conversation-queue-page.test.tsx`, current queue/context Rust tests, and the conversation-queue E2E.

Playback-based transcript suppression is deliberately retired: only sample-based AEC may remove echo. Project selector UI and frontend multi-agent replacement are outside the current first-trial surface; backend scope identity/refusal contracts remain tested. Offline role-routing and Steward contracts remain in Rust.

Startup cancellation, repeat start, retry, final-delivery deduplication/recovery are added to the current capture tests. Backend job leases, terminal refusal, bound queues and cancellation remain in task-queue and queue E2E contracts.

## Original cases

### ambient-voice-capture.test.ts

- attaches a 16 kHz worklet and forwards PCM frames
- returns immediately when capture is already owned or blocked

### ambient-voice-session.test.tsx

- derives capture settings from the conversation policy
- target-speaker alone does not prove playback echo rejection
- marks the microphone ready only after the current ASR session reports ready
- each finalized ASR utterance goes directly to the Qwen turn path
- TTS keeps ASR running and suppresses unverified playback transcripts
- ASR can start while speech playback is already running
- keeps preparing until a pending microphone start is released
- a repeated start request does not invalidate the pending microphone request
- does not invoke the dormant LFM frontend for voice utterances

### ambient-voice-session-failure.test.tsx

- returns to stopped and allows retry after ASR startup fails
- records a microphone permission failure before ASR starts
- releases capture and returns to stopped when ASR stop fails

### larm-voice-drain.test.ts

- stuck final-delivery state has a bounded drain deadline
- OFF during startup cancels immediately instead of waiting for capture to become idle
- turning listening back on invalidates a pending drain

### larm-voice-lifetime.test.tsx

- owns a connection while listening and releases it when disabled

### larm-voice-owner.test.ts

- capture restarts share one connection and stopping during startup rejects the stale capture
- late failure releases the original owner without touching its replacement
- failed startup can be retried after releasing the failed connection
- out-of-order start completion is released even if an earlier end saw no backend owner
- release failure is retried before a replacement can start
- a stale capture for another conversation never gets the new owner's lease
- replacing an owner directly still releases the previous connection first

### reasoning-run.test.ts

- only a live reasoning run opts into replacement on follow-up input
- follow-up preserves options and bounded queues report non-delivery
- input in a different conversation is never added to the old run's replacement queue

### required-context-recovery.test.ts

- keeps the three refusal reasons distinct and non-retryable
- does not turn ordinary provider failures into recovery refusals

### voice-capture-races.test.ts

- overlapping detaches and disposal cannot touch replacement capture resources
- dispose during flush invalidates the old detach before replacement attachment

### voice-capture-resources.test.ts

- capture disposal is idempotent and resolves ASR stop waiters

### voice-final-delivery.test.ts

- deduplicates and retains capacity until downstream delivery is confirmed
- a rejected downstream delivery can be claimed again without duplication
- clear drops pending work and the unmount dedupe set

### world-scope-component.test.tsx

- wr_t20_select_B_keeps_A_answer_attached_to_A_and_deleted_selection_visible

### world-scope.test.ts

- same-name projects use registered identities and preserve original report scope
- deleted targets fail instead of silently switching to another project

