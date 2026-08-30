import { useEffect, useRef, useState } from "react";
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

const VOICE_PROFILE_SAMPLE_RATE = 16_000;

const prompts = [
  "今日は落ち着いて、普段どおりの声で話しています。",
  "この音声は、私の声だけを識別するために使います。",
  "少し声の高さを変えても、自然な話し方を続けます。",
  "机から少し離れた位置でも、はっきり発音します。",
  "最後のサンプルとして、いつもの速さで文章を読みます。",
];

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
  const captureRef = useRef<Capture | null>(null);
  const pendingCaptureRef = useRef<PendingVoiceCapture | null>(null);
  const disposedRef = useRef(false);
  const maximumTimerRef = useRef<number | null>(null);
  const elapsedTimerRef = useRef<number | null>(null);
  const startedAtRef = useRef(0);
  const [captureState, setCaptureState] = useState<"idle" | "starting" | "recording" | "saving">("idle");
  const [elapsedMs, setElapsedMs] = useState(0);
  const [level, setLevel] = useState(0);
  const [message, setMessage] = useState<string | null>(null);
  const playback = useVoiceSamplePlayback(setMessage);

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
      maximumTimerRef.current = window.setTimeout(() => { void stopAndSave(); }, 12_000);
      setCaptureState("recording");
    } catch (cause) {
      await closePendingCapture(pending);
      await closeCapture(false);
      if (disposedRef.current) return;
      setCaptureState("idle");
      setMessage(microphoneErrorMessage(cause));
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
      if (capture.length === 0) throw new Error("音声が記録されませんでした。");
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
      setMessage(`サンプル ${next.sampleCount}/${next.targetSampleCount} を暗号化して保存しました。`);
    } catch (cause) {
      if (!disposedRef.current) setMessage(cause instanceof Error ? cause.message : String(cause));
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
      setMessage(cause instanceof Error ? cause.message : String(cause));
    }
  }

  async function removeSample(sampleId: string) {
    if (!window.confirm("この音声サンプルを削除しますか？")) return;
    try {
      onChanged(await deleteVoiceEnrollmentSample(sampleId));
      setMessage("音声サンプルを削除しました。");
    } catch (cause) {
      void getVoiceProfileSnapshot().then(onChanged).catch(() => undefined);
      setMessage(cause instanceof Error ? cause.message : String(cause));
    }
  }

  async function removeProfile() {
    if (!window.confirm("声プロファイル、暗号化音声、Keychain の鍵をすべて削除しますか？")) return;
    try {
      onChanged(await deleteVoiceProfile());
      setMessage("声プロファイルを削除しました。");
    } catch (cause) {
      void getVoiceProfileSnapshot().then(onChanged).catch(() => undefined);
      setMessage(cause instanceof Error ? cause.message : String(cause));
    }
  }

  const ready = profile.status === "ready";
  const prompt = prompts[Math.min(profile.sampleCount, prompts.length - 1)];
  return <section className="settings-card voice-profile-card">
    <div className="card-title-row"><div><h3>My voice profile</h3><p className="settings-help">登録した声だけを文字起こしへ通す、端末内の話者照合です。</p></div><span className={`voice-profile-status ${ready ? "ready" : "collecting"}`}>{profile.status}</span></div>
    <div className="voice-profile-progress"><strong>{profile.sampleCount} / {profile.targetSampleCount} samples</strong><span>{(profile.totalDurationMs / 1000).toFixed(1)} / {(profile.minimumDurationMs / 1000).toFixed(0)} sec minimum</span></div>
    <p className="voice-enrollment-prompt">「{prompt}」</p>
    <div className="voice-enrollment-controls">
      <button className={captureState === "recording" ? "secondary-button recording" : "secondary-button"} type="button" onClick={() => captureState === "recording" ? void stopAndSave() : void startCapture()} disabled={blocked || playback.playingId !== null || captureState === "starting" || captureState === "saving" || profile.sampleCount >= profile.targetSampleCount || !profile.runtimeAvailable}>{captureState === "starting" ? "マイクを準備中…" : captureState === "saving" ? "照合データを作成中…" : captureState === "recording" ? "録音を停止して保存" : "サンプルを録音"}</button>
      {captureState === "recording" && <><span>{(elapsedMs / 1000).toFixed(1)} sec</span><span className="voice-level" aria-label="Input level"><i style={{ width: `${Math.round(level * 100)}%` }} /></span></>}
    </div>
    {blocked && <p className="provider-test-result error">会話録音、読み上げ、またはミーティングを停止してから登録してください。</p>}
    {!profile.runtimeAvailable && <p className="provider-test-result error">{profile.runtimeMessage}</p>}
    <div className="voice-sample-list">{profile.samples.map((sample) => <div key={sample.id}><span>Sample {sample.ordinal} · {(sample.durationMs / 1000).toFixed(1)} sec · AEC {sample.effectiveAec ? "on" : "off"}</span><div><button className="text-button" type="button" onClick={() => void playback.play(sample.id)} disabled={blocked || playback.playingId !== null || captureState !== "idle"}>{playback.playingId === sample.id ? "Playing…" : "Play"}</button><button className="text-button danger" type="button" onClick={() => void removeSample(sample.id)} disabled={blocked || playback.playingId !== null || captureState !== "idle"}>Delete</button></div></div>)}</div>
    <label className="check-row"><input type="checkbox" checked={profile.filterEnabled} disabled={blocked || playback.playingId !== null || captureState !== "idle" || (!profile.filterEnabled && (!ready || !profile.runtimeAvailable))} onChange={(event) => void toggleFilter(event.target.checked)} />文字起こしを自分の声だけに限定する（判定不能時は送信しない）</label>
    <div className="locked-policy">音声ファイルと話者埋め込みは暗号化して端末内へ保存します。鍵は macOS Keychain に保存し、クラウドへ送信しません。</div>
    <p className="settings-help">これは文字起こし対象を絞る機能で、本人認証や録音・合成音声の検出には使用できません。</p>
    {profile.sampleCount > 0 && <button className="text-button danger" type="button" onClick={() => void removeProfile()} disabled={blocked || playback.playingId !== null || captureState !== "idle"}>Delete entire voice profile</button>}
    {message && <p className={message.includes("保存しました") || message.includes("削除しました") ? "provider-test-result success" : "provider-test-result error"} aria-live="polite">{message}</p>}
  </section>;
}
