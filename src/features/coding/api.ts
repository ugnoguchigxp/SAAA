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
  terminal: z
    .object({
      phase: z.string(),
      cli: z.string(),
      terminal: z.string(),
      verification: z.unknown().optional(),
      questions: z.array(
        z.object({
          questionId: z.string(),
          kind: z.string(),
          state: z.string(),
          input: z.record(z.string(), z.unknown()),
        }),
      ),
    })
    .nullable()
    .optional(),
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
  answer: (
    conversationId: string,
    jobId: string,
    expectedRevision: number,
    questionId: string,
    answer: unknown,
  ) =>
    invoke("answer_terminal_question", {
      conversationId,
      jobId,
      expectedRevision,
      questionId,
      answer,
    }),
  recover: (conversationId: string, jobId: string, expectedRevision: number) =>
    invoke("recover_terminal_job", { conversationId, jobId, expectedRevision }),
  complete: (conversationId: string, jobId: string, expectedRevision: number) =>
    invoke("confirm_terminal_completion", { conversationId, jobId, expectedRevision }),
  progress: (conversationId: string, jobId: string) =>
    invoke("open_terminal_progress", { conversationId, jobId }),
  cancel: (conversationId: string, jobId: string, expectedRevision: number) =>
    invoke("cancel_coding_job", { conversationId, jobId, expectedRevision }),
};
