import { listen } from "@tauri-apps/api/event";
import { useEffect, useRef } from "react";
import { useCommittedCallback } from "../../useCommittedCallback";

export function usePersonalStateForget(onForget: (runs: string[]) => void) {
  const forgotten = useRef(new Set<string>());
  const accept = useCommittedCallback((runs: string[]) => {
    for (const id of runs) forgotten.current.add(id);
    onForget(runs);
  });
  useEffect(() => {
    let disposed = false;
    let stop: (() => void) | undefined;
    void listen<{ runIds: string[] }>("personal-state-forgotten", (event) =>
      accept(event.payload.runIds),
    )
      .then((unlisten) => {
        if (disposed) unlisten();
        else stop = unlisten;
      })
      .catch(() => undefined);
    return () => {
      disposed = true;
      stop?.();
    };
  }, [accept]);
  return forgotten;
}
