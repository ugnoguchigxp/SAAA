import type { RoleRoutingSettings } from "../../lib/roleRoutingTypes";

export function RoleRoutingSection({
  settings,
  onChange,
}: {
  settings: RoleRoutingSettings;
  onChange: (next: RoleRoutingSettings) => void;
}) {
  return (
    <section className="settings-section" aria-label="Role routing">
      <label className="settings-switch">
        <input
          type="checkbox"
          checked={settings.enabled}
          onChange={(event) => onChange({ ...settings, enabled: event.target.checked })}
        />
        <span>モデル役割ルーティングを有効にする</span>
      </label>
      <p className="settings-help">
        有効にすると、応答レシピで指定した reasoner に会話を渡します。設定を保存するまで実行経路は変わりません。
      </p>
      <label className="settings-field">
        <span>選択方法</span>
        <select
          value={settings.selection.mode}
          onChange={(event) =>
            onChange({
              ...settings,
              selection: { ...settings.selection, mode: event.target.value as "rules" | "shadow" },
            })
          }
        >
          <option value="rules">ルール</option>
          <option value="shadow">シャドー評価</option>
        </select>
      </label>
      <label className="settings-switch">
        <input
          type="checkbox"
          checked={settings.learning.enabled}
          onChange={(event) =>
            onChange({
              ...settings,
              learning: { ...settings.learning, enabled: event.target.checked },
            })
          }
        />
        <span>夜間のローカル学習データ作成を有効にする</span>
      </label>
      <p className="settings-help">
        アクターとレシピは設定ドキュメントとして検証されます。対応する Provider と role の割り当てを設定してから有効化してください。
      </p>
    </section>
  );
}
