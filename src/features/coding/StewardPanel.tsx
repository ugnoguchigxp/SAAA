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
  const [tasks, setTasks] = useState<{ taskId: string; loopState: string; goalStatus: string }[]>(
    [],
  );
  async function refresh() {
    try {
      setTasks(await stewardApi.listTasks(conversationId));
    } catch (error) {
      onError(stewardErrorMessage(error));
    }
  }
  useEffect(() => {
    void refresh();
  }, [conversationId]);
  async function registerGoal() {
    if (!workspaceId) return;
    try {
      await stewardApi.registerGoal(conversationId, workspaceId, condition);
      await refresh();
      onError("");
    } catch (error) {
      onError(stewardErrorMessage(error));
    }
  }
  async function withdrawGoal() {
    try {
      await stewardApi.withdraw(conversationId);
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
        自動修正はしません。会話に「{STEWARD_START_TRIGGER}」と送ると、失敗テストの確認だけを依頼します。Memory
        が OFF のときは Goal があっても動きません。
      </p>
      <label>
        成功条件
        <input value={condition} onChange={(event) => setCondition(event.target.value)} />
      </label>
      <button onClick={() => void registerGoal()} disabled={!workspaceId}>
        Goal を登録
      </button>
      <button onClick={() => void withdrawGoal()}>Goal を撤回</button>
      {tasks.map((task) => (
        <p key={task.taskId}>
          {task.goalStatus} / {task.loopState}
        </p>
      ))}
    </details>
  );
}
