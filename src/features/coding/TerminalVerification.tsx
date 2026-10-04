function record(value: unknown): Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value)
    ? (value as Record<string, unknown>)
    : {};
}
export function TerminalVerification({ value }: { value: unknown }) {
  const result = record(value);
  const checks = Array.isArray(result.checks) ? result.checks : [];
  const candidate = record(result.candidate);
  const manual = Array.isArray(candidate.remainingManualChecks)
    ? candidate.remainingManualChecks
    : [];
  return (
    <details>
      <summary>完了確認の結果</summary>
      {typeof candidate.summary === "string" && <p>{candidate.summary}</p>}
      {result.interrupted === true && <p>確認処理が中断されています。結果を確認してください。</p>}
      {checks.length === 0 ? (
        <p>自動の確認処理は実行されていません。</p>
      ) : (
        <ul>
          {checks.map((item, index) => {
            const check = record(item);
            const passed =
              check.exitCode === 0 && check.timeout !== true && check.streamIncomplete !== true;
            return (
              <li key={index}>
                確認 {index + 1}:{" "}
                {passed ? "通過" : check.timeout === true ? "時間切れ" : "確認が必要"}
                {typeof check.error === "string" && check.error && <p>{check.error}</p>}
                {typeof check.output === "string" && check.output && <pre>{check.output}</pre>}
              </li>
            );
          })}
        </ul>
      )}
      {manual.length > 0 && (
        <>
          <p>残っている手動確認</p>
          <ul>
            {manual
              .filter((item): item is string => typeof item === "string")
              .map((item, index) => (
                <li key={index}>{item}</li>
              ))}
          </ul>
        </>
      )}
    </details>
  );
}
