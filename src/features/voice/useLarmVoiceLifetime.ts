import { useEffect, useRef } from "react";
import { endLarmVoice, ownLarmVoice } from "../../lib/larmVoiceRuntime";
import { drainLarmVoice } from "../../lib/larmVoiceDrain";
import type { LarmVoiceOwner } from "../../lib/larmVoiceOwner";

export function useLarmVoiceLifetime(enabled: boolean, conversationId: string | null, busy: () => boolean, onError: (message: string) => void) {
  const active = useRef(enabled);
  const generation = useRef(0);
  const owned = useRef<LarmVoiceOwner | null>(null);
  const latest = useRef({ busy, onError }); latest.current = { busy, onError };
  const failed = () => latest.current.onError("LARM接続の解放を確認できませんでした。");
  const releaseAfterFinal = () => {
    const current = owned.current;
    if (!current) return;
    const revision = ++generation.current;
    void drainLarmVoice(current, {
      cancelled: () => revision !== generation.current || active.current,
      busy: () => latest.current.busy(),
      release: endLarmVoice,
    }).catch(failed);
  };
  useEffect(() => {
    active.current = enabled;
    if (enabled && conversationId) { generation.current++; owned.current = ownLarmVoice(conversationId); }
    else releaseAfterFinal();
  }, [enabled, conversationId]);
  useEffect(() => () => {
    generation.current++;
    const current = owned.current;
    if (current?.conversationId === conversationId) {
      owned.current = null;
      void endLarmVoice(current).catch(failed);
    }
  }, [conversationId]);
  return (next: boolean) => {
    active.current = next;
    if (next && conversationId) { generation.current++; owned.current = ownLarmVoice(conversationId); }
    else releaseAfterFinal();
  };
}
