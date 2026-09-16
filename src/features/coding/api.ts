import { invoke } from "@tauri-apps/api/core";
import { z } from "zod";
import type { CodingSettings } from "../../lib/generated/coding";
export type { CodingSettings };
const job = z.object({
  jobId: z.string(),
  runId: z.string(),
  revision: z.number(),
  state: z.string(),
  workspace: z.string(),
  sessionId: z.string().nullable(),
  delivery: z.string(),
  result: z
    .object({
      summary: z.string().optional(),
      error: z.string().optional(),
      errors: z.number().optional(),
      complete: z.boolean().optional(),
    })
    .nullable(),
});
const snapshot = z.object({
  workspace: z.object({ workspaceId: z.string(), path: z.string() }).nullable(),
  jobs: z.array(job),
});
export type CodingSnapshot = z.infer<typeof snapshot>;
export const codingApi = {
  settings: () => invoke<CodingSettings>("get_coding_settings"),
  save: (settings: CodingSettings) => invoke<void>("save_coding_settings", { settings }),
  probe: () => invoke("probe_coding"),
  snapshot: async (conversationId: string) =>
    snapshot.parse(await invoke("coding_snapshot", { conversationId })),
  workspace: (conversationId: string, path: string) =>
    invoke("register_coding_workspace", { conversationId, path }),
  cancel: (conversationId: string, jobId: string, expectedRevision: number) =>
    invoke("cancel_coding_job", { conversationId, jobId, expectedRevision }),
};
