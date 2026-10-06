import { listen } from "@tauri-apps/api/event";
import { useEffect, useState } from "react";
import { restingExpression, speechExpression } from "./speechExpression";
export { avatarMotions, restingExpression, speechExpression } from "./speechExpression";

// The backend owns chunking, inference and playback. This view only follows its
// audio-ready/end events; user messages and LLM token deltas cannot trigger Laya.
export function useAvatarDecision(conversationId: string, active: boolean) {
  const [state, setState] = useState(restingExpression);
  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | undefined;
    setState(restingExpression);
    if (!active || !conversationId) return;
    void listen<unknown>("conversation-speech-expression", ({ payload }) => {
      if (!disposed) setState((previous) => speechExpression(previous, payload, conversationId));
    })
      .then((stop) => {
        if (disposed) stop();
        else unlisten = stop;
      })
      .catch(() => {
        if (!disposed)
          setState({ ...restingExpression, error: "表情の更新を受信できませんでした。" });
      });
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, [active, conversationId]);
  return { cue: state.cue, error: state.error };
}
