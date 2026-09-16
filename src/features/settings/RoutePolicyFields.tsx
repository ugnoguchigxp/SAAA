import { conversationTimeoutMsFromSecondsInput } from "../../lib/conversationTimeout";
import { useTranslation } from "react-i18next";
import type { ModelProviderSettings } from "../../lib/contracts";
import { Field } from "./SettingsFields";
type Policy = { timeoutMs: number; attemptTimeoutMs?: number; fallbackProviderIds?: string[] };
export function RoutePolicyFields({
  value,
  primary,
  candidates,
  onChange,
  maxTotalMs = 300_000,
}: {
  maxTotalMs?: number;
  value: Policy;
  primary: string | null;
  candidates: ModelProviderSettings[];
  onChange: (value: Policy) => void;
}) {
  const { t } = useTranslation();
  const ids = value.fallbackProviderIds ?? [];
  const available = candidates.filter((p) => p.enabled && p.id !== primary && !ids.includes(p.id));
  return (
    <details>
      <summary>{t("settings.fallback.title")}</summary>
      <div className="settings-form-grid">
        <Field label={t("settings.fallback.total")}>
          <input
            type="number"
            min={1}
            max={maxTotalMs / 1000}
            value={value.timeoutMs / 1000}
            onChange={(e) => {
              const n = conversationTimeoutMsFromSecondsInput(e.target.value);
              if (n !== null && n <= maxTotalMs)
                onChange({
                  ...value,
                  timeoutMs: n,
                  attemptTimeoutMs:
                    value.attemptTimeoutMs === undefined
                      ? undefined
                      : Math.min(value.attemptTimeoutMs, n),
                });
            }}
          />
        </Field>
        <Field label={t("settings.fallback.attempt")}>
          <input
            type="number"
            min={1}
            max={value.timeoutMs / 1000}
            step={0.001}
            value={value.attemptTimeoutMs === undefined ? "" : value.attemptTimeoutMs / 1000}
            placeholder={t("settings.fallback.automatic", {
              seconds: Math.floor(value.timeoutMs / (ids.length + 1)) / 1000,
            })}
            onChange={(e) => {
              const n = e.target.value
                ? conversationTimeoutMsFromSecondsInput(e.target.value)
                : undefined;
              if (n !== null && (n === undefined || n <= value.timeoutMs))
                onChange({ ...value, attemptTimeoutMs: n });
            }}
          />
        </Field>
      </div>
      <ol>
        {ids.map((id, index) => (
          <li key={id}>
            {candidates.find((p) => p.id === id)?.label ?? id}{" "}
            <button
              type="button"
              className="text-button"
              disabled={index === 0}
              onClick={() => {
                const next = [...ids];
                [next[index - 1], next[index]] = [next[index]!, next[index - 1]!];
                onChange({ ...value, fallbackProviderIds: next });
              }}
            >
              {t("settings.fallback.up")}
            </button>
            <button
              type="button"
              className="text-button"
              onClick={() =>
                onChange({ ...value, fallbackProviderIds: ids.filter((p) => p !== id) })
              }
            >
              {t("settings.fallback.remove")}
            </button>
          </li>
        ))}
      </ol>
      <Field label={t("settings.fallback.add")}>
        <select
          value=""
          onChange={(e) => {
            if (e.target.value)
              onChange({ ...value, fallbackProviderIds: [...ids, e.target.value] });
          }}
        >
          <option value="">—</option>
          {available.map((p) => (
            <option key={p.id} value={p.id}>
              {p.label}
            </option>
          ))}
        </select>
      </Field>
      <p className="settings-help">{t("settings.fallback.hint")}</p>
    </details>
  );
}
