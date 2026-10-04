import type { CodingSettings } from "./api";
import { useState } from "react";
export function TerminalSettings({
  settings,
  onChange,
  onValidityChange,
}: {
  settings: CodingSettings;
  onChange: (settings: CodingSettings) => void;
  onValidityChange?: (valid: boolean) => void;
}) {
  const [checks, setChecks] = useState(JSON.stringify(settings.terminalChecks ?? []));
  const [error, setError] = useState("");
  return (
    <section
      className={
        settings.implementationMethod === "terminal"
          ? "coding-method-card selected"
          : "coding-method-card"
      }
    >
      <label className="coding-method-choice">
        <input
          type="radio"
          name="coding-method"
          checked={settings.implementationMethod === "terminal"}
          onChange={() => onChange({ ...settings, implementationMethod: "terminal" })}
        />
        <strong>専用端末で実装</strong>
      </label>
      <label>
        デフォルトの端末
        <select
          value={settings.terminalKind ?? ""}
          onChange={(e) => onChange({ ...settings, terminalKind: e.target.value })}
        >
          <option value="">選択してください</option>
          <option value="kitty">Kitty</option>
          <option value="ghostty">Ghostty</option>
        </select>
      </label>
      <label>
        実装エージェント
        <select
          value={settings.terminalCli ?? ""}
          onChange={(e) => onChange({ ...settings, terminalCli: e.target.value })}
        >
          <option value="">選択してください</option>
          <option value="claude">Claude Code</option>
          <option value="codex">Codex</option>
        </select>
      </label>
      <label>
        CLI実行ファイル（空欄で自動検出）
        <input
          value={settings.terminalExecutable ?? ""}
          onChange={(e) => onChange({ ...settings, terminalExecutable: e.target.value })}
          placeholder="/Users/…/bin/claude"
        />
      </label>
      <label>
        モデル（空欄でCLIの設定を使用）
        <input
          value={settings.terminalModel ?? ""}
          onChange={(e) => onChange({ ...settings, terminalModel: e.target.value })}
        />
      </label>
      <label>
        <input
          type="checkbox"
          checked={settings.terminalAutoAnswer ?? false}
          onChange={(e) => onChange({ ...settings, terminalAutoAnswer: e.target.checked })}
        />{" "}
        元の依頼に明記された回答をSAAAから送る
      </label>
      <label>
        <input
          type="checkbox"
          checked={(settings.terminalRetryLimit ?? 0) > 0}
          onChange={(e) => onChange({ ...settings, terminalRetryLimit: e.target.checked ? 2 : 0 })}
        />{" "}
        確認処理が失敗したら、同じ作業の修正を依頼する（最大2回）
      </label>
      <label>
        完了時に実行する確認処理
        <textarea
          value={checks}
          onChange={(e) => {
            const value = e.target.value;
            setChecks(value);
            try {
              const parsed: unknown = JSON.parse(value);
              if (
                !Array.isArray(parsed) ||
                !parsed.every(
                  (row) =>
                    Array.isArray(row) &&
                    row.length > 0 &&
                    row.every((arg) => typeof arg === "string"),
                )
              )
                throw new Error();
              onChange({ ...settings, terminalChecks: parsed as string[][] });
              setError("");
              onValidityChange?.(true);
            } catch {
              onValidityChange?.(false);
              setError("実行ファイルと引数の配列で入力してください。修正するまで保存できません。");
            }
          }}
          placeholder={'[["bun", "run", "typecheck"], ["bun", "test"]]'}
        />
      </label>
      {error && <p role="alert">{error}</p>}
      <p className="settings-help">
        指定したフォルダーで確認処理を実行します。例: [["bun", "run",
        "typecheck"]]。空の配列なら完了確認をユーザーに依頼します。
      </p>
      <p>
        端末は進捗表示専用です。質問への回答・追加指示・停止はSAAAから操作します。端末を閉じても作業は続きます。
      </p>
    </section>
  );
}
