import { Channel, invoke } from "@tauri-apps/api/core";
import { useEffect, useRef, useState } from "react";
import { stageAudioUpload } from "../../lib/audioIpc";
import { startBrowserVoiceCapture, type BrowserVoiceCapture } from "../../lib/browserVoiceCapture";
import "./providerUnitTestPage.css";

type Capability = "asr" | "tts" | "backchannel" | "llm" | "embedding";
type TestResult = {
  capability: Capability;
  model: string;
  output: string;
  latencyMs: number;
  audioBase64: string | null;
};

const services: Array<{ id: Capability; label: string; description: string; initial: string }> = [
  { id: "asr", label: "ASR", description: "マイクで録音した音声を文字起こしします。", initial: "" },
  {
    id: "tts",
    label: "TTS",
    description: "入力文を音声に変換して再生できます。",
    initial: "こんにちは。音声合成のテストです。",
  },
  {
    id: "backchannel",
    label: "Qwen2B",
    description: "backchannel に入力文を送り、応答を表示します。",
    initial: "短く自己紹介してください。",
  },
  {
    id: "llm",
    label: "Ornith1.5",
    description: "主LLMに入力文を送り、応答を表示します。",
    initial: "短く自己紹介してください。",
  },
  {
    id: "embedding",
    label: "Embedding",
    description: "入力文のベクトル次元と先頭値を確認します。",
    initial: "これは埋め込みのテストです。",
  },
];

export function ProviderUnitTestPage({
  inputDeviceId,
  echoCancellation,
  onOpenSettings,
}: {
  inputDeviceId: string;
  echoCancellation: boolean;
  onOpenSettings: () => void;
}) {
  const [selected, setSelected] = useState<Capability>("asr");
  const [inputs, setInputs] = useState<Record<Capability, string>>(
    Object.fromEntries(services.map((service) => [service.id, service.initial])) as Record<
      Capability,
      string
    >,
  );
  const [results, setResults] = useState<Partial<Record<Capability, TestResult>>>({});
  const [errors, setErrors] = useState<Partial<Record<Capability, string>>>({});
  const [busy, setBusy] = useState<Capability | null>(null);
  const [progress, setProgress] = useState("");
  const [elapsedSeconds, setElapsedSeconds] = useState(0);
  const [recording, setRecording] = useState(false);
  const [recorded, setRecorded] = useState<Float32Array | null>(null);
  const capture = useRef<BrowserVoiceCapture | null>(null);
  const frames = useRef<Float32Array[]>([]);
  const count = useRef(0);
  const audioUrl = useRef<string | null>(null);
  const [previewUrl, setPreviewUrl] = useState<string | null>(null);
  const current = services.find((service) => service.id === selected)!;

  useEffect(
    () => () => {
      void capture.current?.stop();
      if (audioUrl.current) URL.revokeObjectURL(audioUrl.current);
    },
    [],
  );

  useEffect(() => {
    if (!busy) return;
    const started = Date.now();
    const timer = window.setInterval(() => {
      setElapsedSeconds(Math.floor((Date.now() - started) / 1000));
    }, 1000);
    return () => window.clearInterval(timer);
  }, [busy]);

  function fail(capability: Capability, cause: unknown) {
    setErrors((current) => ({ ...current, [capability]: String(cause) }));
  }

  async function stopRecording() {
    const active = capture.current;
    if (active) {
      capture.current = null;
      try {
        await active.stop();
        const samples = new Float32Array(count.current);
        let offset = 0;
        for (const frame of frames.current) {
          samples.set(frame, offset);
          offset += frame.length;
          frame.fill(0);
        }
        frames.current = [];
        count.current = 0;
        setRecorded(samples);
      } catch (cause) {
        fail("asr", cause);
      } finally {
        setRecording(false);
      }
    }
  }

  async function toggleRecording() {
    if (capture.current) {
      await stopRecording();
      return;
    }
    recorded?.fill(0);
    setRecorded(null);
    frames.current = [];
    count.current = 0;
    setErrors((current) => ({ ...current, asr: undefined }));
    try {
      capture.current = await startBrowserVoiceCapture(
        (frame) => {
          if (count.current + frame.length <= 16_000 * 120) {
            frames.current.push(frame.slice());
            count.current += frame.length;
          } else {
            fail("asr", "録音は2分以内にしてください。");
            void stopRecording();
          }
        },
        (reason) => {
          fail("asr", reason);
          void stopRecording();
        },
        inputDeviceId,
        echoCancellation,
      );
      setRecording(true);
    } catch (cause) {
      fail("asr", cause);
    }
  }

  async function run() {
    if (busy || recording) return;
    const capability = selected;
    setBusy(capability);
    setElapsedSeconds(0);
    setProgress(capability === "asr" ? "音声を転送中…" : "接続準備中…");
    setErrors((current) => ({ ...current, [capability]: undefined }));
    setResults((current) => ({ ...current, [capability]: undefined }));
    if (capability === "tts") {
      if (audioUrl.current) URL.revokeObjectURL(audioUrl.current);
      audioUrl.current = null;
      setPreviewUrl(null);
    }
    try {
      let audioUploadId: string | undefined;
      if (capability === "asr") {
        if (!recorded || recorded.length < 1_600) throw new Error("0.1秒以上録音してください。");
        audioUploadId = await stageAudioUpload(recorded, "provider-unit-asr");
      }
      const onProgress = new Channel<{ stage: string }>();
      onProgress.onmessage = ({ stage }) => {
        const messages: Record<string, string> = {
          model_preparing:
            capability === "asr" ? "接続準備中…" : "接続準備中…（初回は最大3分かかります）",
          capacity_waiting: "実行枠が空くのを待っています…",
          semantic_probing:
            capability === "asr"
              ? "接続先の準備を確認中…"
              : "接続先のモデルを準備中…（初回は最大3分かかります）",
          ready: "接続が完了しました。",
          provider_request: capability === "asr" ? "音声を文字起こし中…" : "応答を待っています…",
          releasing: "接続を解放中…",
          terminal_failure: "接続準備に失敗しました。",
        };
        if (messages[stage]) setProgress(messages[stage]);
      };
      const result = await invoke<TestResult>("run_provider_unit_test", {
        input: { capability, text: inputs[capability], audioUploadId },
        onProgress,
      });
      setResults((current) => ({ ...current, [capability]: result }));
      if (capability === "asr") {
        recorded?.fill(0);
        setRecorded(null);
      }
      if (result.audioBase64) {
        const binary = atob(result.audioBase64);
        const bytes = Uint8Array.from(binary, (character) => character.charCodeAt(0));
        audioUrl.current = URL.createObjectURL(new Blob([bytes], { type: "audio/wav" }));
        setPreviewUrl(audioUrl.current);
      }
    } catch (cause) {
      fail(capability, cause);
    } finally {
      setBusy(null);
    }
  }

  return (
    <section className="provider-unit-page" aria-label="単体テスト">
      <header className="provider-unit-header">
        <div>
          <h1>単体テスト</h1>
          <p>
            保存済みのLARM接続とprofileを使い、5つのproviderを1つずつ実行します。テスト結果は会話履歴や設定に保存しません。
          </p>
        </div>
        <button type="button" onClick={onOpenSettings}>
          Provider設定
        </button>
      </header>
      <div className="provider-unit-layout">
        <nav className="provider-unit-list" aria-label="テスト対象">
          {services.map((service) => (
            <button
              key={service.id}
              type="button"
              disabled={busy !== null || (recording && selected !== service.id)}
              aria-current={selected === service.id ? "page" : undefined}
              className={selected === service.id ? "selected" : ""}
              onClick={() => setSelected(service.id)}
            >
              <strong>{service.label}</strong>
              <span>{service.description}</span>
              {results[service.id] && <small>直近のテスト成功</small>}
              {errors[service.id] && <small className="error">直近のテスト失敗</small>}
            </button>
          ))}
        </nav>
        <article className="provider-unit-detail">
          <div className="provider-unit-title">
            <div>
              <small>{selected}</small>
              <h2>{current.label}</h2>
              <p>{current.description}</p>
            </div>
            {results[selected] && <span>成功 · {results[selected].latencyMs} ms</span>}
          </div>
          {selected === "asr" ? (
            <div className="provider-unit-input">
              <button type="button" onClick={() => void toggleRecording()} disabled={busy !== null}>
                {recording ? "録音を停止" : "録音を開始"}
              </button>
              <span role="status">
                {recording
                  ? "録音中…"
                  : recorded
                    ? `録音済み · ${(recorded.length / 16_000).toFixed(1)}秒`
                    : "音声を録音してください"}
              </span>
            </div>
          ) : (
            <label className="provider-unit-input">
              <span>{selected === "tts" ? "読み上げる文" : "入力文"}</span>
              <textarea
                value={inputs[selected]}
                maxLength={4096}
                rows={5}
                onChange={(event) =>
                  setInputs((current) => ({ ...current, [selected]: event.target.value }))
                }
              />
            </label>
          )}
          <button
            className="provider-unit-run"
            type="button"
            disabled={
              busy !== null ||
              recording ||
              (selected === "asr" ? !recorded : !inputs[selected].trim())
            }
            onClick={() => void run()}
          >
            {busy === selected ? "実行中…" : `${current.label}をテスト`}
          </button>
          {busy === selected && <p role="status">{progress}（経過 {elapsedSeconds} 秒）</p>}
          {errors[selected] && (
            <p className="provider-unit-error" role="alert">
              {errors[selected]}
            </p>
          )}
          {results[selected] && (
            <section className="provider-unit-result" aria-live="polite">
              <h3>結果</h3>
              <p>Model: {results[selected].model}</p>
              <pre>{results[selected].output}</pre>
              {selected === "tts" && previewUrl && (
                <audio controls src={previewUrl} aria-label="生成した音声" />
              )}
            </section>
          )}
        </article>
      </div>
    </section>
  );
}
