import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import type { VoiceProfileSnapshot, VoiceSettings } from "../../lib/contracts";
import {
  deleteVoiceEnrollmentSample,
  deleteVoiceProfile,
  getVoiceProfileSnapshot,
  saveVoiceEnrollmentSample,
  setTargetSpeakerFilterEnabled,
} from "../../lib/runtime";
import {
  ensureMicrophoneAudioContextRunning,
  microphoneCaptureConstraints,
  microphoneErrorMessage,
  requestMicrophoneStream,
} from "../../lib/microphone";
import { acquireAudioCapture } from "../../lib/audioCaptureCoordinator";
import { resamplePcm } from "../../lib/audioResampling";
import { mergePcmFrames } from "../../lib/pcm";
import { withTimeout } from "../../lib/promiseTimeout";
import { disposePendingVoiceCapture, type PendingVoiceCapture } from "./pendingVoiceCapture";
import { useVoiceSamplePlayback } from "./useVoiceSamplePlayback";
import { VOICE_ENROLLMENT_AUTO_STOP_MS, VOICE_ENROLLMENT_MINIMUM_SECONDS } from "./voiceEnrollmentPrompts";
import { VoiceTranscriptionScope } from "./VoiceTranscriptionScope";
import { localizeUiMessage, uiMessage } from "../../i18n/presentation";
const VOICE_PROFILE_SAMPLE_RATE = 16_000;
const promptKeys = ["one", "two", "three", "four", "five"] as const;
type Capture = {
  stream: MediaStream;
  context: AudioContext;
  source: MediaStreamAudioSourceNode;
  node: AudioWorkletNode;
  frames: Float32Array[];
  length: number;
  releaseLease: () => void;
  effectiveAec: boolean;
  inputDeviceId: string;
};
const CAPTURE_START_TIMEOUT_MS = 10_000;
type ProfileNotice =
  | { kind: "sampleSaved"; current: number; target: number }
  | { kind: "sampleDeleted" }
  | { kind: "profileDeleted" }
  | { kind: "error"; message: string };
export function VoiceProfileCard({
  voice,
  profile,
  blocked,
  onChanged,
}: {
  voice: VoiceSettings;
  profile: VoiceProfileSnapshot;
  blocked: boolean;
  onChanged: (profile: VoiceProfileSnapshot) => void;
}) {
  const { t } = useTranslation();
  const captureRef = useRef<Capture | null>(null);
  const pendingCaptureRef = useRef<PendingVoiceCapture | null>(null);
  const disposedRef = useRef(false);
  const maximumTimerRef = useRef<number | null>(null);
  const elapsedTimerRef = useRef<number | null>(null);
  const startedAtRef = useRef(0);
  const [captureState, setCaptureState] = useState<"idle" | "starting" | "recording" | "saving">("idle");
  const [elapsedMs, setElapsedMs] = useState(0);
  const [level, setLevel] = useState(0);
  const [message, setMessage] = useState<ProfileNotice | null>(null);
  const playback = useVoiceSamplePlayback((error) => setMessage({ kind: "error", message: error }));
  useEffect(() => {
    disposedRef.current = false;
    return () => {
      disposedRef.current = true;
      void closePendingCapture();
      void closeCapture(false);
    };
  }, []);
  async function startCapture() {
    if (blocked || playback.playingId !== null || captureState !== "idle" || profile.sampleCount >= profile.targetSampleCount || pendingCaptureRef.current || captureRef.current) return;
    const pending: PendingVoiceCapture = { stream: null, context: null, releaseLease: null };
    pendingCaptureRef.current = pending;
    try {
      setCaptureState("starting");
      setMessage(null);
      pending.releaseLease = acquireAudioCapture("voice-enrollment");
      pending.stream = await withTimeout(
        requestMicrophoneStream(microphoneCaptureConstraints(voice.inputDeviceId)),
        CAPTURE_START_TIMEOUT_MS,
        "Microphone startup timed out",
        (lateStream) => lateStream.getTracks().forEach((track) => track.stop()),
      );
      if (disposedRef.current || pendingCaptureRef.current !== pending) {
        await closePendingCapture(pending);
        return;
      }
      pending.context = new AudioContext();
      await withTimeout(pending.context.audioWorklet.addModule("/audio/meeting-processor.js"), CAPTURE_START_TIMEOUT_MS, "Audio processor startup timed out");
      if (disposedRef.current || pendingCaptureRef.current !== pending) {
        await closePendingCapture(pending);
        return;
      }
      const source = pending.context.createMediaStreamSource(pending.stream);
      const node = new AudioWorkletNode(pending.context, "meeting-processor");
      const settings = pending.stream.getAudioTracks()[0]?.getSettings();
      const capture: Capture = {
        stream: pending.stream,
        context: pending.context,
        source,
        node,
        frames: [],
        length: 0,
        releaseLease: pending.releaseLease,
        effectiveAec: settings?.echoCancellation === true,
        inputDeviceId: settings?.deviceId || voice.inputDeviceId,
      };
      pending.stream = null;
      pending.context = null;
      pending.releaseLease = null;
      pendingCaptureRef.current = null;
      captureRef.current = capture;
      node.port.onmessage = (event: MessageEvent<Float32Array>) => {
        if (!(event.data instanceof Float32Array) || captureRef.current !== capture) return;
        capture.frames.push(event.data);
        capture.length += event.data.length;
        const rms = Math.sqrt(event.data.reduce((sum, value) => sum + value * value, 0) / Math.max(1, event.data.length));
        setLevel(Math.min(1, rms * 12));
      };
      source.connect(node);
      node.connect(capture.context.destination);
      await ensureMicrophoneAudioContextRunning(capture.context);
      if (disposedRef.current || captureRef.current !== capture) return;
      startedAtRef.current = performance.now();
      setElapsedMs(0);
      elapsedTimerRef.current = window.setInterval(() => setElapsedMs(performance.now() - startedAtRef.current), 100);
      maximumTimerRef.current = window.setTimeout(() => { void stopAndSave(); }, VOICE_ENROLLMENT_AUTO_STOP_MS);
      setCaptureState("recording");
    } catch (cause) {
      await closePendingCapture(pending);
      await closeCapture(false);
      if (disposedRef.current) return;
      setCaptureState("idle");
      setMessage({ kind: "error", message: microphoneErrorMessage(cause) });
    }
  }
  async function stopAndSave() {
    const capture = captureRef.current;
    if (!capture || captureState === "saving") return;
    setCaptureState("saving");
    setMessage(null);
    try {
      await closeCapture(true);
      if (disposedRef.current) return;
      if (capture.length === 0) throw new Error(uiMessage("voiceProfileNoAudio"));
      const merged = mergePcmFrames(capture.frames, capture.length);
      const normalized = resamplePcm(merged, capture.context.sampleRate, VOICE_PROFILE_SAMPLE_RATE);
      for (const frame of capture.frames) frame.fill(0);
      merged.fill(0);
      capture.frames = [];
      capture.length = 0;
      const next = await saveVoiceEnrollmentSample({
        samples: normalized,
        sampleRate: VOICE_PROFILE_SAMPLE_RATE,
        inputDeviceId: capture.inputDeviceId,
        effectiveAec: capture.effectiveAec,
      }).finally(() => normalized.fill(0));
      if (disposedRef.current) return;
      onChanged(next);
      setMessage({ kind: "sampleSaved", current: next.sampleCount, target: next.targetSampleCount });
    } catch (cause) {
      if (!disposedRef.current) setMessage({ kind: "error", message: cause instanceof Error ? cause.message : String(cause) });
    } finally {
      if (!disposedRef.current) {
        setCaptureState("idle");
        setElapsedMs(0);
        setLevel(0);
      }
    }
  }
  async function closeCapture(preserveFrames: boolean) {
    if (maximumTimerRef.current !== null) window.clearTimeout(maximumTimerRef.current);
    if (elapsedTimerRef.current !== null) window.clearInterval(elapsedTimerRef.current);
    maximumTimerRef.current = null;
    elapsedTimerRef.current = null;
    const capture = captureRef.current;
    captureRef.current = null;
    if (!capture) return;
    capture.node.disconnect();
    capture.source.disconnect();
    capture.stream.getTracks().forEach((track) => track.stop());
    await capture.context.close().catch(() => undefined);
    capture.releaseLease();
    if (!preserveFrames) {
      for (const frame of capture.frames) frame.fill(0);
      capture.frames = [];
      capture.length = 0;
    }
  }
  async function closePendingCapture(pending = pendingCaptureRef.current) {
    if (!pending) return;
    if (pendingCaptureRef.current === pending) pendingCaptureRef.current = null;
    await disposePendingVoiceCapture(pending);
  }
  async function toggleFilter(enabled: boolean) {
    try {
      setMessage(null);
      onChanged(await setTargetSpeakerFilterEnabled(enabled));
    } catch (cause) {
      setMessage({ kind: "error", message: cause instanceof Error ? cause.message : String(cause) });
    }
  }
  async function removeSample(sampleId: string) {
    if (!window.confirm(t("voice.profile.confirmDeleteSample"))) return;
    try {
      onChanged(await deleteVoiceEnrollmentSample(sampleId));
      setMessage({ kind: "sampleDeleted" });
    } catch (cause) {
      void getVoiceProfileSnapshot().then(onChanged).catch(() => undefined);
      setMessage({ kind: "error", message: cause instanceof Error ? cause.message : String(cause) });
    }
  }
  async function removeProfile() {
    if (!window.confirm(t("voice.profile.confirmDeleteProfile"))) return;
    try {
      onChanged(await deleteVoiceProfile());
      setMessage({ kind: "profileDeleted" });
    } catch (cause) {
      void getVoiceProfileSnapshot().then(onChanged).catch(() => undefined);
      setMessage({ kind: "error", message: cause instanceof Error ? cause.message : String(cause) });
    }
  }
  const ready = profile.status === "ready";
  const prompt = t(`voice.profile.prompts.${promptKeys[Math.min(profile.sampleCount, promptKeys.length - 1)]}`);
  const messageText = message?.kind === "sampleSaved"
    ? t("voice.profile.sampleSaved", { current: message.current, target: message.target })
    : message?.kind === "sampleDeleted"
      ? t("voice.profile.sampleDeleted")
      : message?.kind === "profileDeleted"
        ? t("voice.profile.profileDeleted")
        : message?.kind === "error"
          ? localizeUiMessage(t, message.message, "voice")
          : null;
  return <section className="settings-card voice-profile-card">
    <div className="card-title-row"><div><h3>{t("voice.profile.title")}</h3><p className="settings-help">{t("voice.profile.description")}</p></div><span className={`voice-profile-status ${ready ? "ready" : "collecting"}`}>{t(`voice.profile.status.${profile.status}`, { defaultValue: profile.status })}</span></div>
    <div className="voice-profile-progress"><strong>{t("voice.profile.sampleProgress", { current: profile.sampleCount, target: profile.targetSampleCount })}</strong><span>{t("voice.profile.durationProgress", { current: (profile.totalDurationMs / 1000).toFixed(1), minimum: (profile.minimumDurationMs / 1000).toFixed(0) })}</span></div>
    <p className="voice-enrollment-prompt">「{prompt}」</p>
    <p className="settings-help">{t("voice.profile.recordingGuidance", { seconds: VOICE_ENROLLMENT_MINIMUM_SECONDS })}</p>
    <div className="voice-enrollment-controls">
      <button className={captureState === "recording" ? "secondary-button recording" : "secondary-button"} type="button" onClick={() => void startCapture()} disabled={blocked || playback.playingId !== null || captureState !== "idle" || profile.sampleCount >= profile.targetSampleCount || !profile.runtimeAvailable}>{captureState === "starting" ? t("voice.profile.prepareMic") : captureState === "saving" ? t("voice.profile.preparingData") : captureState === "recording" ? t("voice.profile.recordingUntilAutoStop") : t("voice.profile.recordSample")}</button>
      {captureState === "recording" && <><span>{(elapsedMs / 1000).toFixed(1)} {t("common.seconds")}</span><span className="voice-level" aria-label={t("voice.profile.inputLevel")}><i style={{ width: `${Math.round(level * 100)}%` }} /></span></>}
    </div>
    {blocked && <p className="provider-test-result error">{t("voice.profile.blocked")}</p>}
    {!profile.runtimeAvailable && <p className="provider-test-result error">{localizeUiMessage(t, profile.runtimeMessage, "voice")}</p>}
    <div className="voice-sample-list">{profile.samples.map((sample) => <div key={sample.id}><span>{t("voice.profile.sample", { number: sample.ordinal, duration: (sample.durationMs / 1000).toFixed(1), aec: sample.effectiveAec ? t("common.on") : t("common.off") })}</span><div><button className="text-button" type="button" onClick={() => void playback.play(sample.id)} disabled={blocked || playback.playingId !== null || captureState !== "idle"}>{playback.playingId === sample.id ? t("common.playing") : t("common.play")}</button><button className="text-button danger" type="button" onClick={() => void removeSample(sample.id)} disabled={blocked || playback.playingId !== null || captureState !== "idle"}>{t("common.delete")}</button></div></div>)}</div>
    <VoiceTranscriptionScope filterEnabled={profile.filterEnabled} disabled={blocked || playback.playingId !== null || captureState !== "idle"} canEnableFilter={ready && profile.runtimeAvailable} onChange={(enabled) => void toggleFilter(enabled)} />
    <div className="locked-policy">{t("voice.profile.storage")}</div>
    <p className="settings-help">{t("voice.profile.limitation")}</p>
    {profile.sampleCount > 0 && <button className="text-button danger" type="button" onClick={() => void removeProfile()} disabled={blocked || playback.playingId !== null || captureState !== "idle"}>{t("voice.profile.deleteProfile")}</button>}
    {messageText && <p className={message?.kind === "error" ? "provider-test-result error" : "provider-test-result success"} aria-live="polite">{messageText}</p>}
  </section>;
}
