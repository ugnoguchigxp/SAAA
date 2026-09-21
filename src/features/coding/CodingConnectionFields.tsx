import type { CodingSettings } from "./api";
export function CodingConnectionFields({
  settings,
  onChange: setSettings,
}: {
  settings: CodingSettings;
  onChange: (settings: CodingSettings) => void;
}) {
  return (
    <>
      <label>
        接続方式
        <select
          value={settings.profile}
          onChange={(e) =>
            setSettings({
              ...settings,
              version: "0.86.1",
              profile: e.target.value,
              provider:
                e.target.value === "codex-sdk-v1"
                  ? "saaa-codex-sdk"
                  : "openai-codex",
              model: "gpt-5.6-luna",
              sdkExtensionPath:
                e.target.value === "codex-sdk-v1"
                  ? settings.sdkExtensionPath ?? ""
                  : null,
            })
          }
        >
          <option value="trusted-local-v1">pi標準</option>
          <option value="delegated-read-test-macos-v1">委譲調査・テスト（macOS制限）</option>
          <option value="codex-sdk-v1">Codex SDK</option>
        </select>
      </label>
      {settings.profile === "codex-sdk-v1" && (
        <label>
          Codex SDK拡張の絶対パス
          <input
            value={settings.sdkExtensionPath ?? ""}
            onChange={(e) => setSettings({ ...settings, sdkExtensionPath: e.target.value })}
          />
        </label>
      )}
    </>
  );
}
