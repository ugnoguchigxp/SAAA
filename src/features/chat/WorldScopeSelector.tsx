import type { WorldContextStatus } from "../../lib/generated/runtimeEvent";
export function WorldScopeSelector({
  status,
  value,
  onChange,
}: {
  status: WorldContextStatus | null;
  value: string;
  onChange: (key: string) => void;
}) {
  return (
    <aside aria-label="会話の対象">
      <label>
        対象{" "}
        <select value={value} onChange={(event) => onChange(event.target.value)}>
          <option value="">登録済みの実装先を使用</option>
          {status?.choices.map((choice) => (
            <option key={choice.key} value={choice.key}>
              {choice.label}
            </option>
          ))}
          {value && !status?.choices.some((choice) => choice.key === value) && (
            <option value={value}>対象の再確認が必要</option>
          )}
        </select>
      </label>
      {status?.latestScopeKeys.length ? (
        <p>直近の回答の対象: {status.latestScopeKeys.join("、")}</p>
      ) : null}
      {status?.delivery && (
        <p>
          直近のWorld情報: {status.delivery === "sent" ? "送信済み" : "送信記録なし／省略"}
          {status.omissionReason ? `（${status.omissionReason}）` : ""}
        </p>
      )}
    </aside>
  );
}
