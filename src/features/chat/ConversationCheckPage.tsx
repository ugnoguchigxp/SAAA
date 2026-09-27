import {
  useCallback,
  useEffect,
  useRef,
  useState,
  useSyncExternalStore,
  type FormEvent,
} from "react";
import { AppIcon } from "../../components/AppIcon";
import { useArtifactWorkspace } from "./artifacts/ArtifactDrawer";
import { normalizeAnswerUrl } from "./artifacts/answerUrls";
import { MarkdownView } from "./ui/MarkdownView";
import { useLatestMessageScroll } from "./useLatestMessageScroll";
import { listen } from "@tauri-apps/api/event";
import type { ConversationMessage } from "../../lib/contracts";
import {
  conversationAsrSnapshot,
  startConversationAsr,
  stopConversationAsr,
  subscribeConversationAsr,
  setConversationAsrPlaybackActive,
  queueConversationAsrDelivery,
  failConversationAsrDelivery,
  retryConversationAsrDelivery,
} from "../../lib/conversationAsrCapture";
import {
  cancelConversationInput,
  conversationQueueSnapshot,
  enqueueConversationText,
  listMessages,
  replayConversationSpeech,
  startConversationAudioIdle,
  stopConversationAudioIdle,
  type ConversationQueueJob,
} from "../../lib/runtime";
import "./conversationCheckPage.css";

type RouteStage = "dispatch" | "ornith" | "tts" | null;
const routeNodes = [
  { id: "asr", label: "ASR" },
  { id: "ornith", label: "Ornith 1.5" },
  { id: "tts", label: "TTS" },
] as const;

const SOURCE_LINKS_MARKER = "\n\n<!-- saaa:source-links -->\n";

function displayAnswer(content: string) {
  const [answer, appendix] = content.split(SOURCE_LINKS_MARKER, 2);
  const sources =
    appendix?.split("\n").flatMap((line) => {
      const match = /^\[([^\]\r\n]+)\]\((https?:\/\/[^\s)]+)\)$/.exec(line);
      const url = match && normalizeAnswerUrl(match[2]);
      return match && url ? [{ label: match[1], url }] : [];
    }) ?? [];
  return { answer, sources };
}

export function ConversationCheckPage({
  conversationId,
  agentName,
  inputDeviceId,
  echoCancellation,
  listeningEnabled,
  vadSensitivity,
  silenceTimeoutMs,
  onToggleListening,
}: {
  conversationId: string;
  agentName: string;
  providerLabel: string;
  inputDeviceId: string;
  echoCancellation: boolean;
  listeningEnabled: boolean;
  vadSensitivity: "low" | "medium" | "high";
  silenceTimeoutMs: number;
  onToggleListening?: (enabled: boolean) => void;
}) {
  const [messages, setMessages] = useState<ConversationMessage[]>([]);
  const [liveAnswers, setLiveAnswers] = useState<Record<string, string>>({});
  const {
    messageAreaRef: historyRef,
    messageContentRef: historyContentRef,
    updateFollowLatest: updateHistoryFollow,
  } = useLatestMessageScroll(conversationId, false);
  const artifacts = useArtifactWorkspace();
  const [text, setText] = useState("");
  const [sending, setSending] = useState(false);
  const [stage, setStage] = useState<RouteStage>(null);
  const [jobs, setJobs] = useState<ConversationQueueJob[]>([]);
  const [speechPlaying, setSpeechPlaying] = useState(false);
  const lastSpeech = jobs
    .slice()
    .reverse()
    .find((job) => job.kind === "speech");
  const lastReplySource = lastSpeech ? "Ornith 1.5" : null;
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const audio = useSyncExternalStore(subscribeConversationAsr, conversationAsrSnapshot);
  const pendingId = useRef<string | null>(null);
  const refreshGeneration = useRef(0);
  const audioResponseQueue = useRef(Promise.resolve());
  const autoStartAttempted = useRef(false);
  const latestJob = jobs
    .slice()
    .reverse()
    .find((job) => job.kind !== "progress_speech");
  const queueError = latestJob?.state === "failed" ? latestJob.error : null;
  const latestAsrError = audio.entries[0]?.status === "failed" ? audio.entries[0].error : null;
  const failedDelivery = audio.entries.find(
    (entry) =>
      entry.status === "failed" &&
      entry.provider &&
      entry.text?.trim() &&
      new TextEncoder().encode(entry.text).length <= 4096 &&
      !entry.deliveryQueued,
  );
  const transcribing = audio.entries.some((entry) => entry.status === "transcribing");
  const asrActive = audio.phase === "starting" || audio.phase === "recording";
  const recognizing = asrActive && Boolean(audio.speechDetected || audio.interimText || transcribing);
  const displayedText = text || audio.interimText;
  const pendingJob = jobs
    .slice()
    .reverse()
    .find((job) => job.state === "queued" || job.state === "running");
  const interruptedSpeech = jobs
    .slice()
    .reverse()
    .find((job) => job.kind === "speech" && job.state === "interrupted");
  const status =
    stage === "dispatch"
      ? "Ornith 1.5 に接続中"
      : stage === "ornith"
        ? "Ornith 1.5 が回答を作成中"
        : stage === "tts"
          ? "TTS で回答を再生中"
          : pendingJob
            ? "回答を処理待ち"
            : asrActive
              ? recognizing
                ? "音声を認識中"
                : "音声を待っています"
              : audio.phase === "starting"
                ? "マイクを準備中"
                : "マイクは停止中";

  const refreshQueue = useCallback(async () => {
    const generation = ++refreshGeneration.current;
    const [snapshot, page] = await Promise.all([
      conversationQueueSnapshot(),
      listMessages(conversationId, null),
    ]);
    if (generation !== refreshGeneration.current) return;
    setJobs(snapshot.jobs);
    setSpeechPlaying(snapshot.speechPlaying);
    setMessages(
      page.messages.filter((message) => message.role === "user" || message.role === "assistant"),
    );
    setLiveAnswers((current) => Object.fromEntries(Object.entries(current).filter(([inputId]) =>
      !page.messages.some((message) => message.id === `reply_${inputId}`) &&
      !snapshot.jobs.some((job) => job.key === inputId && ["failed", "cancelled"].includes(job.state)),
    )));
  }, [conversationId]);

  useEffect(() => {
    let active = true;
    const started = startConversationAudioIdle();
    void started.catch((cause) => {
      if (active) setError(String(cause));
    });
    return () => {
      active = false;
      void started.catch(() => {}).then(stopConversationAudioIdle);
    };
  }, []);

  useEffect(() => {
    let active = true;
    let unlisten: (() => void) | undefined;
    setLoading(true);
    const reload = () => {
      void refreshQueue()
        .catch((cause) => {
          if (active) setError(String(cause));
        })
        .finally(() => {
          if (active) setLoading(false);
        });
    };
    void listen("conversation-queue-updated", () => {
      if (active) reload();
    })
      .then((stop) => {
        if (active) {
          unlisten = stop;
          reload();
        } else stop();
      })
      .catch((cause) => {
        if (active) {
          setError(String(cause));
          setLoading(false);
        }
      });
    return () => {
      active = false;
      refreshGeneration.current += 1;
      unlisten?.();
    };
  }, [refreshQueue]);

  useEffect(() => {
    let active = true;
    let unlisten: (() => void) | undefined;
    void listen<{ inputId: string; text: string }>("conversation-answer-delta", ({ payload }) => {
      if (!active || !payload.text) return;
      setLiveAnswers((current) => ({
        ...current,
        [payload.inputId]: (current[payload.inputId] ?? "") + payload.text,
      }));
    }).then((stop) => {
      if (active) unlisten = stop;
      else stop();
    }).catch((cause) => setError(String(cause)));
    return () => { active = false; unlisten?.(); };
  }, []);

  useEffect(() => {
    setConversationAsrPlaybackActive(speechPlaying);
    const active =
      jobs.find((job) => job.state === "running" && job.kind === "user_input") ??
      jobs.find((job) => job.state === "running" && job.kind === "speech");
    setStage(
      active?.kind === "user_input"
        ? "ornith"
        : active?.kind === "speech"
          ? "tts"
          : active
            ? "dispatch"
            : null,
    );
  }, [jobs, speechPlaying]);

  useEffect(() => {
    if (!listeningEnabled || autoStartAttempted.current) return;
    autoStartAttempted.current = true;
    void startConversationAsr(inputDeviceId, echoCancellation, vadSensitivity, silenceTimeoutMs);
  }, [listeningEnabled, inputDeviceId, echoCancellation, vadSensitivity, silenceTimeoutMs]);

  const send = useCallback(
    async (content: string, inputId: string): Promise<boolean> => {
      if (!content.trim()) return false;
      if (new TextEncoder().encode(content).length > 4096) {
        setText(content);
        setError("入力が4096バイトを超えています。短くしてから送信してください。");
        return false;
      }
      pendingId.current = inputId;
      setText(content);
      setSending(true);
      setError(null);
      try {
        await enqueueConversationText(inputId, content);
        void refreshQueue().catch((cause) => setError(String(cause)));
        setText((current) => (current === content ? "" : current));
        pendingId.current = null;
        return true;
      } catch (cause) {
        const message = String(cause);
        setError(
          message.includes("larm_authentication_failed")
            ? "LARM の認証に失敗しました。保存済みの接続設定と Ornith の Provider 認証を確認してください。"
            : message,
        );
        return false;
      } finally {
        setSending(false);
      }
    },
    [refreshQueue],
  );

  useEffect(() => {
    for (const capture of audio.entries.slice().reverse()) {
      if (capture.status !== "completed" || !capture.text?.trim() || capture.deliveryQueued)
        continue;
      if (!queueConversationAsrDelivery(capture.id)) continue;
      const content = capture.text;
      audioResponseQueue.current = audioResponseQueue.current.then(async () => {
        if (!(await send(content, capture.id)))
          failConversationAsrDelivery(
            capture.id,
            "会話の処理キューへ送信できませんでした。再送してください。",
          );
      });
    }
  }, [audio.entries, send]);

  async function submit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    if (sending || !displayedText.trim()) return;
    const content = displayedText;
    const inputId = pendingId.current ?? crypto.randomUUID();
    audioResponseQueue.current = audioResponseQueue.current.then(async () => {
      await send(content, inputId);
    });
    await audioResponseQueue.current;
  }

  function retryAsrDelivery(id: string) {
    const content = retryConversationAsrDelivery(id);
    if (!content) return;
    audioResponseQueue.current = audioResponseQueue.current.then(async () => {
      if (!(await send(content, id)))
        failConversationAsrDelivery(
          id,
          "会話の処理キューへ送信できませんでした。再送してください。",
        );
    });
  }

  const toggleRecording = () => {
    if (onToggleListening) {
      onToggleListening(audio.phase !== "recording");
      return;
    }
    void (audio.phase === "recording"
      ? stopConversationAsr()
      : startConversationAsr(inputDeviceId, echoCancellation, vadSensitivity, silenceTimeoutMs));
  };

  return (
    <section className="conversation-check" aria-label="会話">
      <header className="conversation-check-header">
        <h1>会話</h1>
      </header>
      <div
        ref={historyRef}
        className="conversation-check-history"
        aria-live="polite"
        onScroll={updateHistoryFollow}
      >
        <div ref={historyContentRef} className="conversation-check-history-content">
          {loading && <p>会話を読み込み中…</p>}
          {!loading && messages.length === 0 && (
            <p>マイクを有効にするか、メッセージを入力してください。</p>
          )}
          {messages.map((message) => {
            const displayed =
              message.role === "assistant"
                ? displayAnswer(message.content)
                : { answer: message.content, sources: [] };
            return (
              <article key={message.id} className={`conversation-check-message ${message.role}`}>
                <strong>{message.role === "user" ? "あなた" : agentName}</strong>
                {message.role === "assistant"
                  ? <MarkdownView text={displayed.answer} displayMode="inline" />
                  : <p>{displayed.answer}</p>}
                {displayed.sources.length > 0 && (
                  <div className="conversation-check-sources" aria-label="出典">
                    {displayed.sources.map((source) => (
                      <a
                        key={source.url}
                        href={source.url}
                        onClick={(event) => {
                          if (!artifacts) return;
                          event.preventDefault();
                          artifacts.openSource({
                            conversationId,
                            url: source.url,
                            title: source.label,
                          });
                        }}
                      >
                        {source.label}
                      </a>
                    ))}
                  </div>
                )}
              </article>
            );
          })}
          {Object.entries(liveAnswers).map(([inputId, content]) => (
            <article key={`stream_${inputId}`} className="conversation-check-message assistant streaming">
              <strong>{agentName}</strong>
              <MarkdownView text={content} displayMode="inline" />
            </article>
          ))}
          {(stage === "dispatch" || stage === "ornith") && (
            <div className="conversation-thinking" role="status" aria-label="思考中">
              <div className="llm-thinking-indicator" aria-hidden="true"><span /><span /><span /></div>
              <span>{status}</span>
            </div>
          )}
        </div>
      </div>
      {(error || queueError) && (
        <p role="alert" className="conversation-check-error">
          会話処理: {error || queueError}
        </p>
      )}
      {(audio.error || latestAsrError) && (
        <p role="alert" className="conversation-check-error">
          音声認識: {audio.error || latestAsrError}
        </p>
      )}
      {failedDelivery && (
        <button type="button" onClick={() => retryAsrDelivery(failedDelivery.id)}>
          認識済みの発話を再送
        </button>
      )}
      <div className="conversation-route" aria-label="回答の経路">
        <div className="conversation-route-track">
          {routeNodes.map((node, index) => (
            <div className="conversation-route-segment" key={node.id}>
              {index > 0 && (
                <span className="conversation-route-link" aria-hidden="true">
                  →
                </span>
              )}
              <div
                className={`conversation-route-node${(node.id === "asr" ? asrActive && !stage : stage === node.id) ? " active" : ""}`}
              >
                <span className="conversation-route-lamp" aria-hidden="true" />
                <span>{node.label}</span>
              </div>
            </div>
          ))}
        </div>
        <p className="conversation-route-status" role="status">
          {status}
          {!stage && lastReplySource ? ` · 前回回答: ${lastReplySource}` : ""}
        </p>
        {pendingJob && (
          <button
            type="button"
            onClick={() =>
              void cancelConversationInput(pendingJob.key)
                .then(refreshQueue)
                .catch((cause) => setError(String(cause)))
            }
          >
            この依頼を中止
          </button>
        )}
        {interruptedSpeech && (
          <button
            type="button"
            onClick={() =>
              void replayConversationSpeech(interruptedSpeech.key)
                .then(refreshQueue)
                .catch((cause) => setError(String(cause)))
            }
          >
            未再生の回答を読み上げる
          </button>
        )}
      </div>
      <form
        className="composer conversation-check-composer"
        onSubmit={(event) => void submit(event)}
      >
        <div className="composer-row">
          <button
            className={audio.phase === "recording" ? "voice-button recording" : "voice-button"}
            type="button"
            aria-label={audio.phase === "recording" ? "録音を停止" : "録音を開始"}
            aria-pressed={audio.phase === "recording"}
            disabled={audio.phase === "starting" || audio.phase === "stopping"}
            onClick={toggleRecording}
          >
            <AppIcon name={audio.phase === "recording" ? "stop" : "mic"} />
          </button>
          <div
            className={`voice-activity-indicator${asrActive ? " listening" : " paused"}${recognizing ? " detecting" : ""}`}
            role="img"
            aria-label={recognizing ? "音声を認識中" : asrActive ? "音声を待機中" : "マイク停止中"}
          >
            {Array.from({ length: 7 }, (_, index) => <span key={index} />)}
          </div>
          <textarea
            rows={1}
            aria-label="プロンプト全文"
            value={displayedText}
            onChange={(event) => {
              setText(event.currentTarget.value);
              pendingId.current = null;
            }}
            onKeyDown={(event) => {
              if ((event.metaKey || event.ctrlKey) && event.key === "Enter")
                event.currentTarget.form?.requestSubmit();
            }}
            placeholder="メッセージを入力、または話しかけてください"
          />
          <div className="composer-end">
            <button
              className="send-button"
              type="submit"
              aria-label="送信"
              disabled={
                sending ||
                !displayedText.trim() ||
                new TextEncoder().encode(displayedText).length > 4096
              }
            >
              <AppIcon name="send" />
            </button>
          </div>
        </div>
      </form>
    </section>
  );
}
