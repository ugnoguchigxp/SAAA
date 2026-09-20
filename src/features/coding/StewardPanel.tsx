import { useEffect, useState } from "react";
import { stewardApi, stewardErrorMessage, STEWARD_START_TRIGGER } from "./stewardApi";

export function StewardPanel({
  conversationId,
  workspaceId,
  onError,
}: {
  conversationId: string;
  workspaceId?: string;
  onError: (message: string) => void;
}) {
  const [condition, setCondition] = useState("tests pass");
  const [confirmed, setConfirmed] = useState(false);
  const [tasks, setTasks] = useState<
    {
      taskId: string;
      loopState: string;
      goalStatus: string;
      goalId: string;
      summary: string;
      verifier: string;
      workspaceId: string;
      operations: string;
      budgetRuns: number;
      budgetMs: number;
      notify: string;
    }[]
  >([]);
  async function refresh() {
    try {
      setTasks(await stewardApi.listTasks(conversationId));
    } catch (error) {
      onError(stewardErrorMessage(error));
    }
  }
  useEffect(() => {
    void refresh();
    // Terminal reports are delivered from the durable outbox.  Polling is only
    // a UI wake-up; it is not the source of task state.
    const timer = window.setInterval(() => void refresh(), 2_000);
    return () => window.clearInterval(timer);
  }, [conversationId]);
  async function registerGoal() {
    if (!workspaceId || !confirmed) return;
    try {
      await stewardApi.registerGoal(conversationId, workspaceId, condition);
      await refresh();
      onError("");
    } catch (error) {
      onError(stewardErrorMessage(error));
    }
  }
  async function withdrawGoal(goalId?: string) {
    try {
      await stewardApi.withdraw(conversationId, goalId);
      await refresh();
      onError("");
    } catch (error) {
      onError(stewardErrorMessage(error));
    }
  }
  return (
    <details>
      <summary>執事 Goal</summary>
      <p>
        自動修正はしません。会話に「{STEWARD_START_TRIGGER}
        」と送ると、失敗テストの確認だけを依頼します。Memory が OFF のときは Goal
        があっても動きません。
      </p>
      <label>
        成功条件
        <input value={condition} onChange={(event) => setCondition(event.target.value)} />
      </label>
      <label>
        <input
          type="checkbox"
          checked={confirmed}
          onChange={(event) => setConfirmed(event.target.checked)}
        />
        対象ワークスペースで、読み取りと既存テスト実行だけを最大3回・60秒まで許可します。修正・ネットワーク・任意シェルは許可しません。
      </label>
      <button onClick={() => void registerGoal()} disabled={!workspaceId || !confirmed}>
        Goal を登録
      </button>
      <button onClick={() => void withdrawGoal()}>最新の Goal を撤回</button>
      {tasks.map((task) => (
        <p key={task.taskId}>
          {task.summary || task.goalId} / 対象: {task.workspaceId} / 操作: {task.operations} / 予算:{" "}
          {task.budgetRuns}回・{task.budgetMs}ms / 完了条件: {task.verifier} / 通知: {task.notify} /{" "}
          {task.goalStatus} / {task.loopState}
          {task.goalStatus === "active" && (
            <button onClick={() => void withdrawGoal(task.goalId)}>この Goal を撤回</button>
          )}
        </p>
      ))}
    </details>
  );
}
