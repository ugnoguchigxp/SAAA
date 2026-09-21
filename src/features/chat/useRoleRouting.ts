import { useCallback, useEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import {
  decideRoutingProposal,
  getRoutingSnapshot,
  replayRoutingEvents,
} from "../../lib/roleRoutingApi";
import { cancelRoutingRoot } from "../../lib/roleRoutingControls";
import type { RoutingEventRecord, RoutingSnapshot } from "../../lib/generated/runtimeEvent";
import {
  advanceRoutingEventCursors,
  mergeRoutingEventRecords,
  type RoutingEventCursors,
} from "./routingEventReplay";

const emptySnapshot: RoutingSnapshot = {
  active: null,
  queued: [],
  recentRootIds: [],
  proposals: [],
};

/** Projects only the durable routing receipt; actor selection stays in the backend. */
export function useRoleRouting(conversationId: string | null) {
  const [snapshot, setSnapshot] = useState<RoutingSnapshot>(emptySnapshot);
  const [events, setEvents] = useState<RoutingEventRecord[]>([]);
  const [cancellingRootId, setCancellingRootId] = useState<string | null>(null);
  const [decidingProposalId, setDecidingProposalId] = useState<string | null>(null);
  const [proposalError, setProposalError] = useState<string | null>(null);
  const cursorsRef = useRef<RoutingEventCursors>(new Map());
  const eventsRef = useRef<RoutingEventRecord[]>([]);
  const refreshChainRef = useRef<Promise<void>>(Promise.resolve());
  const conversationRef = useRef(conversationId);
  const refresh = useCallback(
    (rootHint?: string) => {
      const synchronize = async () => {
        if (!conversationId) {
          setSnapshot(emptySnapshot);
          return;
        }
        const nextSnapshot = await getRoutingSnapshot(conversationId);
        if (conversationRef.current !== conversationId) return;
        const roots = new Set<string>();
        if (rootHint) roots.add(rootHint);
        if (nextSnapshot.active) roots.add(nextSnapshot.active.rootId);
        for (const root of nextSnapshot.queued) roots.add(root.rootId);
        for (const rootId of nextSnapshot.recentRootIds) roots.add(rootId);
        const replayed = (
          await Promise.all(
            [...roots].map((rootId) =>
              replayRoutingEvents(rootId, cursorsRef.current.get(rootId) ?? 0n),
            ),
          )
        ).flat();
        if (conversationRef.current !== conversationId) return;
        const advanced = advanceRoutingEventCursors(cursorsRef.current, replayed);
        cursorsRef.current = new Map(
          [...advanced].filter(([rootId]) => roots.has(rootId)),
        );
        eventsRef.current = mergeRoutingEventRecords(eventsRef.current, replayed)
          .filter((event) => roots.has(event.rootId))
          .slice(-256);
        setEvents(eventsRef.current);
        setSnapshot(nextSnapshot);
      };
      const result = refreshChainRef.current.catch(() => undefined).then(synchronize);
      refreshChainRef.current = result.catch(() => undefined);
      return result;
    },
    [conversationId],
  );
  useEffect(() => {
    conversationRef.current = conversationId;
    cursorsRef.current = new Map();
    eventsRef.current = [];
    refreshChainRef.current = Promise.resolve();
    setEvents([]);
    if (!conversationId) {
      setSnapshot(emptySnapshot);
      return;
    }
    let disposed = false;
    let unlisten: (() => void) | undefined;
    let timer: number | undefined;
    // Establish the live subscription first. A notification racing with the initial snapshot
    // carries its root id, so even a just-completed root can still be replayed durably.
    void listen<{ rootId?: string }>("role-routing-updated", (event) => {
      void refresh(event.payload.rootId).catch(() => setSnapshot(emptySnapshot));
    }).then((next) => {
      if (disposed) {
        next();
        return;
      }
      unlisten = next;
      void refresh().catch(() => setSnapshot(emptySnapshot));
      timer = window.setInterval(() => {
        void refresh().catch(() => setSnapshot(emptySnapshot));
      }, 3_000);
    });
    return () => {
      disposed = true;
      unlisten?.();
      if (timer !== undefined) window.clearInterval(timer);
    };
  }, [conversationId, refresh]);
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
  const decideProposal = useCallback(
    async (proposalId: string, candidateId: string, approve: boolean) => {
      setDecidingProposalId(proposalId);
      setProposalError(null);
      try {
        await decideRoutingProposal(proposalId, candidateId, approve);
        await refresh();
      } catch (error) {
        setProposalError(error instanceof Error ? error.message : "提案の更新に失敗しました");
      } finally {
        setDecidingProposalId(null);
      }
    },
    [refresh],
  );
  return {
    snapshot,
    events,
    cancellingRootId,
    decidingProposalId,
    proposalError,
    cancel,
    decideProposal,
    refresh: () => refresh(),
  };
}
