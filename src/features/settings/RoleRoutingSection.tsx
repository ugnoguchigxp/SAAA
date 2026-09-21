import type { RoleRoutingSettings } from "../../lib/roleRoutingTypes";
import type { CodexAgentSettings, ModelProvidersSettings } from "../../lib/contracts";
import type { RoutingLearningSnapshot } from "../../lib/generated/runtimeEvent";
import {
  getRoutingLearningSnapshot,
  rollbackAdaptiveArtifact,
  runRoutingLearningOnce,
} from "../../lib/roleRoutingApi";
import { useEffect, useState } from "react";

export function RoleRoutingSection({
  settings,
  providers,
  codex,
  onChange,
}: {
  settings: RoleRoutingSettings;
  providers: ModelProvidersSettings;
  codex: CodexAgentSettings;
  onChange: (next: RoleRoutingSettings) => void;
}) {
  const [learning, setLearning] = useState<RoutingLearningSnapshot | null>(null);
  const [learningBusy, setLearningBusy] = useState(false);
  const [learningUnavailable, setLearningUnavailable] = useState(false);
  const [rollbackId, setRollbackId] = useState<string | null>(null);
  useEffect(() => {
    void getRoutingLearningSnapshot()
      .then(setLearning)
      .catch(() => setLearningUnavailable(true));
  }, []);
  const roleNames = [
    ["frontend", "フロントエンド"],
    ["reasoner", "Reasoner"],
    ["advanced", "Advanced"],
    ["reviewer", "Reviewer"],
    ["premium", "Premium"],
    ["toolSpecialist", "Tool specialist"],
  ] as const;
  const availableProviders = providers.providers.filter(
    (provider) =>
      provider.enabled &&
      (provider.kind === "openai-compatible" ||
        provider.kind === "agent-session" ||
        provider.kind === "dynamic-lan"),
  );

  function prepareActors() {
    const providerActors = availableProviders.map((provider) => ({
      id: `provider-${provider.id}`,
      label: provider.label,
      aliases: [],
      transport: "provider" as const,
      providerId: provider.id,
      model: null,
      location: provider.location,
      resourceGroup: provider.location === "local" ? "local-llm" : "cloud-llm",
      maxInputBytes: 65_536,
      capabilities: ["reason", "tools"],
    }));
    const codexActor =
      codex.enabled && codex.health === "ready"
        ? [
            {
              id: "codex-sol",
              label: "Codex Sol",
              aliases: ["sol"],
              transport: "codex_sdk" as const,
              providerId: null,
              model: "gpt-5.6-sol",
              location: "cloud" as const,
              resourceGroup: "codex-sdk",
              maxInputBytes: 65_536,
              capabilities: ["reason", "tools", "review"],
            },
          ]
        : [];
    const actors = [...providerActors, ...codexActor];
    const reasoner = settings.roles.reasoner ?? actors[0]?.id ?? null;
    onChange({
      ...settings,
      actors,
      roles: { ...settings.roles, reasoner },
      recipes: settings.recipes.length
        ? settings.recipes
        : reasoner
          ? [{ id: "respond-reasoner", action: "respond", roles: ["reasoner"], enabled: true }]
          : [],
    });
  }
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
        有効にすると、応答レシピで指定した reasoner
        に会話を渡します。設定を保存するまで実行経路は変わりません。
      </p>
      <button type="button" className="secondary-button" onClick={prepareActors}>
        利用可能な Provider からアクターを準備
      </button>
      {settings.actors.length > 0 && (
        <div className="settings-field-group">
          {roleNames.map(([role, label]) => (
            <label className="settings-field" key={role}>
              <span>{label}</span>
              <select
                value={settings.roles[role] ?? ""}
                onChange={(event) =>
                  onChange({
                    ...settings,
                    roles: { ...settings.roles, [role]: event.target.value || null },
                  })
                }
              >
                <option value="">未割当</option>
                {settings.actors.map((actor) => (
                  <option key={actor.id} value={actor.id}>
                    {actor.label}
                  </option>
                ))}
              </select>
            </label>
          ))}
        </div>
      )}
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
        アクターとレシピは設定ドキュメントとして検証されます。対応する Provider と role
        の割り当てを設定してから有効化してください。
      </p>
      <div className="settings-field-group" aria-label="学習状況">
        <span>学習状況</span>
        {learningUnavailable ? (
          <p className="settings-help">学習状況を読み取れません。</p>
        ) : learning ? (
          <p className="settings-help">
            保留 {learning.dirtyRoots.toString()}件 / 利用可能dataset{" "}
            {learning.readyDatasets.toString()}件 / shadow artifact{" "}
            {learning.activeArtifacts.toString()}件
          </p>
        ) : (
          <p className="settings-help">読み込み中…</p>
        )}
        <button
          type="button"
          className="secondary-button"
          disabled={!settings.learning.enabled || learningBusy}
          onClick={() => {
            setLearningBusy(true);
            setLearningUnavailable(false);
            void runRoutingLearningOnce()
              .then(setLearning)
              .catch(() => setLearningUnavailable(true))
              .finally(() => setLearningBusy(false));
          }}
        >
          {learningBusy ? "学習データを作成中…" : "今すぐ学習データを作成"}
        </button>
      </div>
      <label className="settings-switch">
        <input
          type="checkbox"
          checked={settings.adaptiveImprovement.enabled}
          onChange={(event) =>
            onChange({
              ...settings,
              adaptiveImprovement: {
                ...settings.adaptiveImprovement,
                enabled: event.target.checked,
              },
            })
          }
        />
        <span>検証済みの改善を次回の選択に使う</span>
      </label>
      {settings.adaptiveImprovement.enabled && (
        <div className="settings-field-group">
          {(
            [
              ["providerRecipe", "応答レシピ"],
              ["tool", "ツール候補"],
              ["plan", "委任作業の手順"],
              ["notification", "通知方法"],
            ] as const
          ).map(([domain, label]) => (
            <label className="settings-switch" key={domain}>
              <input
                type="checkbox"
                checked={settings.adaptiveImprovement[domain]}
                onChange={(event) =>
                  onChange({
                    ...settings,
                    adaptiveImprovement: {
                      ...settings.adaptiveImprovement,
                      [domain]: event.target.checked,
                    },
                  })
                }
              />
              <span>{label}</span>
            </label>
          ))}
          <p className="settings-help">
            明示した訂正はこの設定にかかわらず優先されます。改善は、同じ条件で確認済みの候補だけに適用され、いつでもオフに戻せます。
          </p>
          {learning?.adaptiveArtifacts.length ? (
            <div className="settings-field-group" aria-label="検証済みの改善状況">
              <span>検証済みの改善状況</span>
              {learning.adaptiveArtifacts.map((artifact) => {
                const domain = {
                  provider_recipe: "応答レシピ",
                  tool: "ツール候補",
                  plan: "委任作業の手順",
                  notification: "通知方法",
                }[artifact.domain] ?? artifact.domain;
                const score = artifact.bestObservedScore === null
                  ? "記録なし"
                  : `${Math.round(artifact.bestObservedScore * 100)}%`;
                return (
                  <div className="settings-help" key={artifact.id}>
                    <p>
                      {domain} / {artifact.scopeKey}: {artifact.reason}
                    </p>
                    <p>
                      使える結果 {artifact.eligibleExamples.toString()}件 / 最良の観測結果 {score}
                      {artifact.policyRevision === null
                        ? ""
                        : ` / 適用版 ${artifact.policyRevision.toString()}`}
                    </p>
                    {artifact.state === "active" && (
                      <button
                        type="button"
                        className="secondary-button"
                        disabled={rollbackId !== null}
                        onClick={() => {
                          setRollbackId(artifact.id);
                          setLearningUnavailable(false);
                          void rollbackAdaptiveArtifact(artifact.id)
                            .then(setLearning)
                            .catch(() => setLearningUnavailable(true))
                            .finally(() => setRollbackId(null));
                        }}
                      >
                        {rollbackId === artifact.id ? "ルールへ戻しています…" : "この改善をルールへ戻す"}
                      </button>
                    )}
                  </div>
                );
              })}
            </div>
          ) : (
            <p className="settings-help">
              まだ検証中または有効な改善はありません。十分な比較結果がそろうまで、これまでのルールを使います。
            </p>
          )}
        </div>
      )}
    </section>
  );
}
