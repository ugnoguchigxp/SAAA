import { useEffect, useRef, useState } from "react";
import {
  EMPTY_STREAMING_TEXT,
  StreamingTextBuffer,
  type StreamingTextProjection,
} from "./streamingTextBuffer";
import { recordFirstPlainPaint, recordPlainCommit } from "./streamingPerformance";

export function useStreamingTextProjection() {
  const [streamingText, setStreamingText] = useState<StreamingTextProjection>(EMPTY_STREAMING_TEXT);
  const bufferRef = useRef(new StreamingTextBuffer());
  const updateRef = useRef<{ kind: "frame" | "timeout"; id: number } | null>(null);

  function cancelUpdate() {
    const pending = updateRef.current;
    if (!pending) return;
    if (pending.kind === "frame") cancelAnimationFrame(pending.id);
    else window.clearTimeout(pending.id);
    updateRef.current = null;
  }

  function resetStreamingText() {
    cancelUpdate();
    bufferRef.current = new StreamingTextBuffer();
    setStreamingText(EMPTY_STREAMING_TEXT);
  }

  function appendStreamingText(runId: string, delta: string) {
    bufferRef.current.append(delta);
    if (updateRef.current) return;
    const commit = () => {
      updateRef.current = null;
      setStreamingText(bufferRef.current.snapshot());
      if (recordPlainCommit(runId)) requestAnimationFrame(() => recordFirstPlainPaint(runId));
    };
    updateRef.current = document.visibilityState === "hidden"
      ? { kind: "timeout", id: window.setTimeout(commit, 100) }
      : { kind: "frame", id: requestAnimationFrame(commit) };
  }

  function hasStreamingText() {
    return !bufferRef.current.isEmpty();
  }

  useEffect(() => () => cancelUpdate(), []);
  return { streamingText, resetStreamingText, appendStreamingText, hasStreamingText };
}
