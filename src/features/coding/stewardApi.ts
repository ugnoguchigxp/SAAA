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
});
const stewardRegister = z.object({
  goalId: z.string(),
  delegationId: z.string(),
  status: z.string(),
});
export const STEWARD_START_TRIGGER = "テストを確認して";
export function stewardErrorMessage(error: unknown): string {
  const text = String(error);
  if (text.includes("active_goal_exists"))
    return "同じ会話に有効な Goal が既にあります。先に撤回してください。";
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
      notify: "both" | "silent" | "speak";
    },
  ) =>
    stewardRegister.parse(
      await invoke("register_steward_goal", { conversationId, workspaceId, ...input }),
    ),
  withdraw: (conversationId: string, goalId?: string) =>
    goalId
      ? invoke("work_withdraw", { conversationId, goalId })
      : invoke("withdraw_steward_delegation", { conversationId }),
  listTasks: async (conversationId: string) =>
    z.array(stewardTask).parse(await invoke("list_steward_tasks", { conversationId })),
};
