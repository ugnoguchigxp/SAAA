import { useState } from "react";
import { useTranslation } from "react-i18next";
import type { SecuritySettings } from "../../lib/contracts";
import { backupDatabase, exportDiagnostics } from "../../lib/runtime";
import { localizeUiMessage } from "../../i18n/presentation";
export function SecuritySection({
  security,
  onChange,
}: {
  security: SecuritySettings;
  onChange: (value: SecuritySettings) => void;
}) {
  const { t } = useTranslation();
  const [message, setMessage] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  async function run(action: typeof backupDatabase | typeof exportDiagnostics) {
    try {
      const result = await action();
      setMessage(result.path);
      setError(null);
    } catch (cause) {
      setMessage(null);
      setError(cause instanceof Error ? cause.message : String(cause));
    }
  }
  return (
    <div className="settings-stack">
      <section className="settings-card">
        <h3>{t("settings.security.credentials")}</h3>
        <p>{t("settings.security.credentialsDescription")}</p>
        <div className="locked-policy">{t("settings.security.storagePolicy")}</div>
      </section>
      <section className="settings-card">
        <h3>{t("settings.security.runtimePolicy")}</h3>
        <label className="check-row">
          <input
            type="checkbox"
            checked={security.localOnlyWhenSelected}
            onChange={(event) =>
              onChange({ ...security, localOnlyWhenSelected: event.target.checked })
            }
          />
          {t("settings.security.noCloudFallback")}
        </label>
        <label className="check-row">
          <input type="checkbox" checked={security.diagnosticsRedaction} disabled />
          {t("settings.security.diagnosticsRedaction")}
        </label>
      </section>
      <section className="settings-card">
        <h3>{t("settings.security.dataOperations")}</h3>
        <div className="provider-card-footer">
          <span>
            {error
              ? localizeUiMessage(t, error, "settings")
              : (message ?? t("settings.security.noApiKeysInExports"))}
          </span>
          <div>
            <button
              className="text-button"
              type="button"
              onClick={() => void run(exportDiagnostics)}
            >
              {t("settings.security.exportDiagnostics")}
            </button>
            <button className="text-button" type="button" onClick={() => void run(backupDatabase)}>
              {t("settings.security.backupDatabase")}
            </button>
          </div>
        </div>
      </section>
    </div>
  );
}
