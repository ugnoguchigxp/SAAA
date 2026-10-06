import { useCallback, useEffect, useRef, useState } from "react";
import type {
  BlocklistEntry,
  ProfileDraft,
  WebSearchMode,
  WorkerAgentDetail,
  WorkerAgentSummary,
  WorkerTaskSummary,
} from "../../lib/generated/workerAgents";
import * as api from "./api";

const TASK_LIMIT = 50;
const BLOCKLIST_LIMIT = 200;

function message(cause: unknown) {
  return cause instanceof Error ? cause.message : String(cause);
}

export function useWorkerAgents() {
  const [agents, setAgents] = useState<WorkerAgentSummary[]>([]);
  const [tasks, setTasks] = useState<WorkerTaskSummary[]>([]);
  const [blocklist, setBlocklist] = useState<BlocklistEntry[]>([]);
  // There is no getter for the mode: it is unknown until the user picks one in this session.
  const [mode, setMode] = useState<WebSearchMode | null>(null);
  const [selected, setSelected] = useState<string | null>(null);
  const [detail, setDetail] = useState<WorkerAgentDetail | null>(null);
  const [loaded, setLoaded] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const mounted = useRef(true);
  const selectedRef = useRef<string | null>(null);
  selectedRef.current = selected;

  const refreshAgents = useCallback(async () => {
    setAgents(await api.listWorkerAgents());
    const id = selectedRef.current;
    if (id) setDetail(await api.getWorkerAgent(id));
  }, []);
  const refreshTasks = useCallback(async () => setTasks(await api.listWorkerTasks(TASK_LIMIT)), []);
  const refreshBlocklist = useCallback(
    async () => setBlocklist(await api.listWorkerUrlBlocklist(BLOCKLIST_LIMIT)),
    [],
  );

  /** Runs one operation, then refreshes the data it affects; failures surface as `error`. */
  const attempt = useCallback(async (operation: () => Promise<unknown>) => {
    setBusy(true);
    setError(null);
    try {
      await operation();
      return true;
    } catch (cause) {
      if (mounted.current) setError(message(cause));
      return false;
    } finally {
      if (mounted.current) setBusy(false);
    }
  }, []);

  const reload = useCallback(
    () =>
      attempt(() => Promise.all([refreshAgents(), refreshTasks(), refreshBlocklist()])).then(
        (ok) => {
          // A failed load must not render as "no agents": stay not-loaded and show the error.
          if (ok && mounted.current) setLoaded(true);
          return ok;
        },
      ),
    [attempt, refreshAgents, refreshTasks, refreshBlocklist],
  );

  useEffect(() => {
    mounted.current = true;
    void reload();
    return () => {
      mounted.current = false;
    };
  }, [reload]);

  return {
    agents,
    tasks,
    blocklist,
    mode,
    selected,
    detail,
    loaded,
    busy,
    error,
    reload,
    selectAgent: (profileId: string | null) =>
      attempt(async () => {
        selectedRef.current = profileId;
        setSelected(profileId);
        setDetail(profileId ? await api.getWorkerAgent(profileId) : null);
      }),
    setEnabled: (profileId: string, enabled: boolean) =>
      attempt(async () => {
        await api.setWorkerAgentEnabled(profileId, enabled);
        await refreshAgents();
      }),
    approve: (profileId: string, revisionId: string, definitionHash: string) =>
      attempt(async () => {
        await api.approveWorkerAgentRevision(profileId, revisionId, definitionHash);
        await refreshAgents();
      }),
    saveDraft: (draft: ProfileDraft) =>
      attempt(async () => {
        await api.saveWorkerAgentDraft(draft);
        await refreshAgents();
      }),
    chooseMode: (next: WebSearchMode) =>
      attempt(async () => {
        await api.setWorkerWebSearchMode(next);
        setMode(next);
      }),
    cancelTask: (taskId: string) =>
      attempt(async () => {
        await api.cancelWorkerTask(taskId);
        await refreshTasks();
      }),
    removeBlocked: (urlHash: string) =>
      attempt(async () => {
        await api.removeWorkerUrlBlocklist(urlHash);
        await refreshBlocklist();
      }),
    refreshTasks: () => attempt(refreshTasks),
  };
}

export type WorkerAgentsState = ReturnType<typeof useWorkerAgents>;
