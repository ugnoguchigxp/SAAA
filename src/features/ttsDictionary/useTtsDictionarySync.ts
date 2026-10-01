import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { useCallback, useEffect, useRef, useState } from "react";

export type DictionaryEntry = { written: string; spoken: string };

export function useTtsDictionarySync(onError: (message: string) => void) {
  const [entries, setEntries] = useState<DictionaryEntry[]>([]);
  const live = useRef(false);
  const sequence = useRef(0);
  const errorHandler = useRef(onError);
  errorHandler.current = onError;
  const reload = useCallback(async () => {
    const request = ++sequence.current;
    const items = await invoke<DictionaryEntry[]>("list_tts_dictionary");
    if (live.current && request === sequence.current) setEntries(items);
    return items;
  }, []);

  useEffect(() => {
    live.current = true;
    let active = true;
    let unsubscribe: (() => void) | undefined;
    let queued = false;
    const refresh = () => {
      if (queued || !active) return;
      queued = true;
      queueMicrotask(() => {
        queued = false;
        if (active)
          void reload().catch((cause) => {
            if (active) errorHandler.current(String(cause));
          });
      });
    };
    // Subscribe before loading so changes during the initial read are not missed.
    void listen("tts-dictionary-changed", refresh)
      .then((stop) => {
        if (!active) stop();
        else unsubscribe = stop;
      })
      .catch((cause) => {
        if (active) errorHandler.current(`辞書の更新通知を受信できません: ${String(cause)}`);
      })
      .finally(() => {
        if (active) refresh();
      });
    window.addEventListener("focus", refresh);
    const visible = () => {
      if (document.visibilityState === "visible") refresh();
    };
    document.addEventListener("visibilitychange", visible);
    return () => {
      active = false;
      live.current = false;
      // eslint-disable-next-line react-hooks/exhaustive-deps -- This counter invalidates requests, not a rendered node.
      ++sequence.current;
      unsubscribe?.();
      window.removeEventListener("focus", refresh);
      document.removeEventListener("visibilitychange", visible);
    };
  }, [reload]);
  return { entries, reload };
}
