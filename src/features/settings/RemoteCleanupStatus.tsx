import { z } from "zod";
const operationsSchema = z.array(
  z.object({
    id: z.string(),
    complete: z.boolean(),
    phases: z.record(z.string(), z.string()),
  }),
);
const labels: Record<string, string> = {
  attempts: "推論の終了",
  views: "回答用の入力",
  runtime: "メモリ内の状態",
  snapshots: "保存された推論状態",
  registry: "登録情報",
  sources: "送信した原文",
  audit: "監査用データ",
};
export function RemoteCleanupStatus({ operations }: { operations: unknown }) {
  const parsed = operationsSchema.safeParse(operations);
  if (!parsed.success || parsed.data.length === 0) return null;
  return (
    <details>
      <summary>遠隔消去の確認内訳</summary>
      {parsed.data.map((operation) => (
        <div key={operation.id}>
          <p>{operation.complete ? "消去確認済み" : "消去確認が残っています"}</p>
          <ul>
            {Object.entries(labels).map(([phase, label]) => (
              <li key={phase}>
                {label}: {operation.phases[phase] === "absent" ? "消去確認済み" : "未確認"}
              </li>
            ))}
          </ul>
        </div>
      ))}
    </details>
  );
}
