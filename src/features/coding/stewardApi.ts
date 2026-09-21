import { invoke } from "@tauri-apps/api/core";
import { z } from "zod";

const stewardTask = z.object({
  taskId: z.string(),
  loopState: z.string(),
  dedupeKey: z.string(),
  codingJobId: z.string().nullable(),
  goalStatus: z.string(),
  goalId: z.string(),
  summary: z.string(),
  verifier: z.string(),
  workspaceId: z.string(),
  operations: z.string(),
  budgetRuns: z.number(),
  budgetMs: z.number(),
  notify: z.string(),
  deliveryState: z.string().nullable(),
  speechState: z.string().nullable(),
  artifactRefs: z.array(z.string()),
  verifierOutcome: z.string().nullable().optional(),
  evidenceReason: z.string().nullable().optional(),
});
const stewardRegister = z.object({
  goalId: z.string(),
  delegationId: z.string(),
  status: z.string(),
});
export type StewardNotify = "both" | "silent" | "speak";
export const STEWARD_START_TRIGGER = "テストを確認して";
export function stewardErrorMessage(error: unknown): string {
  const text = String(error);
  if (text.includes("active_goal_limit"))
    return "この会話では有効な Goal は最大 8 件です。不要な Goal を撤回してから登録してください。";
  if (text.includes("workspace_required"))
    return "執事 Goal を登録するには実装先ワークスペースが必要です。";
  if (text.includes("steward_register_invalid")) return "成功条件を入力してください。";
  return text;
}
export const stewardApi = {
  registerGoal: async (
    conversationId: string,
    workspaceId: string,
    input: {
      successCondition: string;
      summary: string;
      verifier: "test_report_obtained" | "tests_pass" | "user_confirmation_required";
      operations: "read" | "test_run" | "read_test";
      budgetRuns: number;
      budgetMs: number;
      notify: StewardNotify;
    },
  ) =>
    stewardRegister.parse(
      await invoke("register_steward_goal", { conversationId, workspaceId, ...input }),
    ),
  withdraw: (conversationId: string, goalId?: string) =>
    goalId
      ? invoke("work_withdraw", { conversationId, goalId })
      : invoke("withdraw_steward_delegation", { conversationId }),
  amendNotification: (conversationId: string, goalId: string, notify: StewardNotify) =>
    invoke("work_amend", { conversationId, goalId, notify }),
  listTasks: async (conversationId: string) =>
    z.array(stewardTask).parse(await invoke("list_steward_tasks", { conversationId })),
  listGoals: async (conversationId: string) =>
    z
      .object({
        goals: z.array(
          z.object({
            goalId: z.string(),
            summary: z.string(),
            authorityStatus: z.string(),
            progress: z.string(),
            workspaceId: z.string(),
            operations: z.string(),
            verifier: z.string(),
            budgetRuns: z.number(),
            budgetMs: z.number(),
            notify: z.string(),
            awaitingReason: z.string().nullable(),
            reportRevision: z.number(),
            unsupportedProfile: z.boolean(),
          }),
        ),
        revision: z.number(),
      })
      .parse(await invoke("list_steward_goals", { conversationId })),
  confirmProposal: (
    conversationId: string,
    proposalId: string,
    expectedRevision: number,
    displayDigest: string,
    start: boolean,
  ) =>
    invoke("work_confirm", {
      conversationId,
      proposalId,
      expectedRevision,
      displayDigest,
      start,
    }),
  registerRecipe: (recipe: {
    name: string;
    target: string;
    cwd: string;
    argv: string[];
    envAllow: string[];
    outputDir: string;
    timeoutMs: number;
  }) => invoke("register_steward_recipe", { recipe }),
};
