import { useTranslation } from "react-i18next";
import type { LlmRequestOptions } from "../../lib/settingsTypes";
import { Field } from "./SettingsFields";

export function LlmRequestFields({
  value,
  dynamic = false,
  onChange,
}: {
  value?: LlmRequestOptions;
  dynamic?: boolean;
  onChange: (value: LlmRequestOptions) => void;
}) {
  const { t } = useTranslation();
  const options = value ?? {
    tokenLimit: "auto",
    reasoning: "auto",
    tools: true,
    streaming: !dynamic,
  };
  return (
    <details>
      <summary>{t("settings.compatibility.title")}</summary>
      <div className="settings-form-grid">
        <Field label={t("settings.compatibility.tokenLimit")}>
          <select
            value={options.tokenLimit}
            onChange={(e) =>
              onChange({
                ...options,
                tokenLimit: e.target.value as LlmRequestOptions["tokenLimit"],
              })
            }
          >
            <option value="auto">{t("settings.compatibility.auto")}</option>
            <option value="legacy">max_tokens</option>
            <option value="completion">max_completion_tokens</option>
          </select>
        </Field>
        <Field label="reasoning_effort">
          <select
            value={options.reasoning}
            onChange={(e) =>
              onChange({ ...options, reasoning: e.target.value as LlmRequestOptions["reasoning"] })
            }
          >
            <option value="auto">{t("settings.compatibility.auto")}</option>
            <option value="supported">{t("settings.compatibility.supported")}</option>
            <option value="unsupported">{t("settings.compatibility.omit")}</option>
          </select>
        </Field>
        <Field label={t("settings.compatibility.tools")}>
          <input
            type="checkbox"
            checked={options.tools}
            onChange={(e) => onChange({ ...options, tools: e.target.checked })}
          />
        </Field>
        <Field label={t("settings.compatibility.streaming")}>
          <input
            type="checkbox"
            checked={options.streaming}
            onChange={(e) => onChange({ ...options, streaming: e.target.checked })}
          />
        </Field>
      </div>
      <p className="settings-help">{t("settings.compatibility.hint")}</p>
    </details>
  );
}
