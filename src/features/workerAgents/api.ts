import { invoke } from "@tauri-apps/api/core";
import type {
  BlocklistEntry,
  ProfileDraft,
  RevisionSummary,
  SkillDraft,
  SkillSaved,
  WebSearchMode,
  WorkerAgentDetail,
  WorkerAgentSummary,
  WorkerTaskSummary,
} from "../../lib/generated/workerAgents";

export const listWorkerAgents = () => invoke<WorkerAgentSummary[]>("list_worker_agents");

export const getWorkerAgent = (profileId: string) =>
  invoke<WorkerAgentDetail>("get_worker_agent", { profileId });

export const saveWorkerAgentDraft = (draft: ProfileDraft) =>
  invoke<RevisionSummary>("save_worker_agent_draft", { draft });

export const approveWorkerAgentRevision = (
  profileId: string,
  revisionId: string,
  definitionHash: string,
) =>
  invoke<WorkerAgentSummary>("approve_worker_agent_revision", {
    profileId,
    revisionId,
    definitionHash,
  });

export const setWorkerAgentEnabled = (profileId: string, enabled: boolean) =>
  invoke<WorkerAgentSummary>("set_worker_agent_enabled", { profileId, enabled });

export const saveWorkerSkill = (draft: SkillDraft) =>
  invoke<SkillSaved>("save_worker_skill", { draft });

export const setWorkerWebSearchMode = (mode: WebSearchMode) =>
  invoke<WebSearchMode>("set_worker_web_search_mode", { mode });

export const listWorkerTasks = (limit: number) =>
  invoke<WorkerTaskSummary[]>("list_worker_tasks", { limit });

export const cancelWorkerTask = (taskId: string) => invoke<void>("cancel_worker_task", { taskId });

export const listWorkerUrlBlocklist = (limit: number, after?: string) =>
  invoke<BlocklistEntry[]>("list_worker_url_blocklist", after ? { limit, after } : { limit });

export const removeWorkerUrlBlocklist = (urlHash: string) =>
  invoke<boolean>("remove_worker_url_blocklist", { urlHash });

/** Task states that can still be cancelled. */
export const ACTIVE_TASK_STATES = ["accepted", "running", "verifying"] as const;
export const isActiveTask = (state: string) =>
  (ACTIVE_TASK_STATES as readonly string[]).includes(state);
