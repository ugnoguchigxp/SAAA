import { useCallback, useEffect, useState } from "react";
import {
  stewardApi,
  stewardErrorMessage,
  STEWARD_START_TRIGGER,
  type StewardNotify,
} from "./stewardApi";

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
  const [summary, setSummary] = useState("失敗テストを調査する");
  const [verifier, setVerifier] = useState<
    "test_report_obtained" | "tests_pass" | "user_confirmation_required"
  >("test_report_obtained");
  const [operations, setOperations] = useState<"read" | "test_run" | "read_test">("read_test");
  const [budgetRuns, setBudgetRuns] = useState(3);
  const [budgetMs, setBudgetMs] = useState(60_000);
  const [notify, setNotify] = useState<StewardNotify>("both");
  const [confirmed, setConfirmed] = useState(false);
  const [notificationChanges, setNotificationChanges] = useState<Record<string, StewardNotify>>({});
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
      deliveryState: string | null;
      speechState: string | null;
      artifactRefs: string[];
    }[]
  >([]);
  const refresh = useCallback(async () => {
    try {
      setTasks(await stewardApi.listTasks(conversationId));
    } catch (error) {
      onError(stewardErrorMessage(error));
    }
  }, [conversationId, onError]);
  useEffect(() => {
    void refresh();
    // Terminal reports are delivered from the durable outbox.  Polling is only
    // a UI wake-up; it is not the source of task state.
    const timer = window.setInterval(() => void refresh(), 2_000);
    return () => window.clearInterval(timer);
  }, [refresh]);
  async function registerGoal() {
    if (!workspaceId || !confirmed) return;
    try {
      await stewardApi.registerGoal(conversationId, workspaceId, {
        successCondition: condition,
        summary,
        verifier,
        operations,
        budgetRuns,
        budgetMs,
        notify,
      });
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
  async function amendNotification(goalId: string, current: StewardNotify) {
    const next = notificationChanges[goalId] ?? current;
    if (next === current) return;
    try {
      await stewardApi.amendNotification(conversationId, goalId, next);
      setNotificationChanges((changes) => {
        const remaining = { ...changes };
        delete remaining[goalId];
        return remaining;
      });
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
        仕事の要約
        <input value={summary} onChange={(event) => setSummary(event.target.value)} />
      </label>
      <label>
        成功条件
        <input value={condition} onChange={(event) => setCondition(event.target.value)} />
      </label>
      <label>
        検証
        <select
          value={verifier}
          onChange={(event) => setVerifier(event.target.value as typeof verifier)}
        >
          <option value="test_report_obtained">テスト結果を取得</option>
          <option value="tests_pass">テスト成功を確認</option>
          <option value="user_confirmation_required">ユーザー確認が必要</option>
        </select>
      </label>
      <label>
        許可する操作
        <select
          value={operations}
          onChange={(event) => setOperations(event.target.value as typeof operations)}
        >
          <option value="read">読み取りだけ</option>
          <option value="test_run">既存テストだけ</option>
          <option value="read_test">読み取りと既存テスト</option>
        </select>
      </label>
      <label>
        最大実行回数
        <input
          type="number"
          min={1}
          max={16}
          value={budgetRuns}
          onChange={(event) => setBudgetRuns(Number(event.target.value))}
        />
      </label>
      <label>
        最大時間（ms）
        <input
          type="number"
          min={1}
          max={3600000}
          value={budgetMs}
          onChange={(event) => setBudgetMs(Number(event.target.value))}
        />
      </label>
      <label>
        通知
        <select value={notify} onChange={(event) => setNotify(event.target.value as typeof notify)}>
          <option value="both">表示と読み上げ</option>
          <option value="silent">表示のみ</option>
          <option value="speak">読み上げ優先</option>
        </select>
      </label>
      <label>
        <input
          type="checkbox"
          checked={confirmed}
          onChange={(event) => setConfirmed(event.target.checked)}
        />
        上記の対象・操作・予算・検証・通知条件でのみ実行を許可します。修正・ネットワーク・任意シェルは許可しません。
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
          {task.deliveryState && <> / 配信: {task.deliveryState}</>}
          {task.speechState && <> / 音声: {task.speechState}</>}
          {task.artifactRefs.length > 0 && <> / 成果物: {task.artifactRefs.join(", ")}</>}
          {task.goalStatus === "active" && (
            <>
              <label>
                通知の変更
                <select
                  value={notificationChanges[task.goalId] ?? task.notify}
                  onChange={(event) =>
                    setNotificationChanges((changes) => ({
                      ...changes,
                      [task.goalId]: event.target.value as StewardNotify,
                    }))
                  }
                >
                  <option value="both">表示と読み上げ</option>
                  <option value="silent">表示のみ</option>
                  <option value="speak">読み上げ優先</option>
                </select>
              </label>
              <button
                onClick={() =>
                  void amendNotification(
                    task.goalId,
                    notificationChanges[task.goalId] ?? (task.notify as StewardNotify),
                  )
                }
                disabled={
                  !notificationChanges[task.goalId] ||
                  notificationChanges[task.goalId] === task.notify
                }
              >
                通知を変更
              </button>
              <button onClick={() => void withdrawGoal(task.goalId)}>この Goal を撤回</button>
            </>
          )}
        </p>
      ))}
    </details>
  );
}
