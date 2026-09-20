import { useCallback, useEffect, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { getRoutingSnapshot } from "../../lib/roleRoutingApi";
import { cancelRoutingRoot } from "../../lib/roleRoutingControls";
import type { RoutingSnapshot } from "../../lib/generated/runtimeEvent";

const emptySnapshot: RoutingSnapshot = { active: null, queued: [] };

/** Projects only the durable routing receipt; actor selection stays in the backend. */
export function useRoleRouting(conversationId: string | null) {
  const [snapshot, setSnapshot] = useState<RoutingSnapshot>(emptySnapshot);
  const [cancellingRootId, setCancellingRootId] = useState<string | null>(null);
  const refresh = useCallback(async () => {
    if (!conversationId) return setSnapshot(emptySnapshot);
    setSnapshot(await getRoutingSnapshot(conversationId));
  }, [conversationId]);
  useEffect(() => {
    void refresh().catch(() => setSnapshot(emptySnapshot));
    if (!conversationId) return;
    const timer = window.setInterval(() => {
      void refresh().catch(() => setSnapshot(emptySnapshot));
    }, 3_000);
    return () => window.clearInterval(timer);
  }, [conversationId, refresh]);
  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | undefined;
    void listen("role-routing-updated", () => {
      void refresh().catch(() => setSnapshot(emptySnapshot));
    }).then((next) => {
      if (disposed) next();
      else unlisten = next;
    });
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, [refresh]);
  const cancel = useCallback(
    async (rootId: string) => {
      setCancellingRootId(rootId);
      try {
        await cancelRoutingRoot(rootId);
        await refresh();
      } finally {
        setCancellingRootId(null);
      }
    },
    [refresh],
  );
  return { snapshot, cancellingRootId, cancel, refresh };
}
