import { useMemo } from "react";
import { useTranslation } from "react-i18next";
import type { RegionalPreferencesSettings } from "../../lib/contracts";
import { setDisplayLanguagePreference } from "../../i18n";
import { availableTimeZones, CURRENCY_CODES, systemTimeZone } from "../../lib/regionalPreferences";
import { Field, Metric } from "./SettingsFields";
import { DEFAULT_AGENT_NAME } from "./settingsDefaults";
import type { SettingsDraft } from "./settingsDraft";

export function SettingsGeneralSection({
  draft,
  onChange,
}: {
  draft: SettingsDraft;
  onChange: (draft: SettingsDraft) => void;
}) {
  const { t, i18n } = useTranslation();
  const timeZones = useMemo(availableTimeZones, []);
  const localTimeZone = systemTimeZone();
  const currencyNames = useMemo(
    () => new Intl.DisplayNames([i18n.resolvedLanguage ?? "en"], { type: "currency" }),
    [i18n.resolvedLanguage],
  );
  const enabledProviders = draft.providers.providers.filter(
    (provider) => provider.enabled && provider.kind !== "dynamic-lan",
  ).length;
  function changeRegional<K extends keyof RegionalPreferencesSettings>(
    key: K,
    value: RegionalPreferencesSettings[K],
  ) {
    onChange({ ...draft, regional: { ...draft.regional, [key]: value } });
  }
  return (
    <div className="settings-stack">
      <section className="settings-card">
        <h3>{t("settings.general.regionalPreferences")}</h3>
        <p>{t("settings.general.regionalPreferencesDescription")}</p>
        <div className="settings-form-grid">
          <Field label={t("settings.general.displayLanguage")}>
            <select
              value={draft.regional.language}
              onChange={(event) => {
                const language = event.currentTarget
                  .value as RegionalPreferencesSettings["language"];
                changeRegional("language", language);
                void setDisplayLanguagePreference(language);
              }}
            >
              <option value="system">{t("settings.general.systemLanguage")}</option>
              <option value="ja">{t("common.japanese")}</option>
              <option value="en">{t("common.english")}</option>
            </select>
          </Field>
          <Field label={t("settings.general.timeZone")}>
            <select
              value={draft.regional.timeZone}
              onChange={(event) => changeRegional("timeZone", event.currentTarget.value)}
            >
              <option value="system">
                {t("settings.general.systemTimeZone", { timeZone: localTimeZone })}
              </option>
              {timeZones.map((timeZone) => (
                <option key={timeZone} value={timeZone}>
                  {timeZone}
                </option>
              ))}
            </select>
          </Field>
          <Field label={t("settings.general.lengthUnit")}>
            <select
              value={draft.regional.lengthUnit}
              onChange={(event) =>
                changeRegional(
                  "lengthUnit",
                  event.currentTarget.value as RegionalPreferencesSettings["lengthUnit"],
                )
              }
            >
              <option value="metric">{t("settings.general.metric")}</option>
              <option value="imperial">{t("settings.general.imperial")}</option>
            </select>
          </Field>
          <Field label={t("settings.general.weightUnit")}>
            <select
              value={draft.regional.weightUnit}
              onChange={(event) =>
                changeRegional(
                  "weightUnit",
                  event.currentTarget.value as RegionalPreferencesSettings["weightUnit"],
                )
              }
            >
              <option value="kilogram">{t("settings.general.kilogram")}</option>
              <option value="pound">{t("settings.general.pound")}</option>
            </select>
          </Field>
          <Field label={t("settings.general.currency")}>
            <select
              value={draft.regional.currency}
              onChange={(event) =>
                changeRegional(
                  "currency",
                  event.currentTarget.value as RegionalPreferencesSettings["currency"],
                )
              }
            >
              {CURRENCY_CODES.map((currency) => (
                <option key={currency} value={currency}>
                  {currency} — {currencyNames.of(currency) ?? currency}
                </option>
              ))}
            </select>
          </Field>
        </div>
      </section>
      <section className="settings-card">
        <h3>{t("settings.general.identity")}</h3>
        <div className="settings-form-grid">
          <Field label={t("settings.general.agentName")}>
            <input
              value={draft.codex.agentName}
              maxLength={80}
              placeholder={DEFAULT_AGENT_NAME}
              onChange={(event) =>
                onChange({ ...draft, codex: { ...draft.codex, agentName: event.target.value } })
              }
            />
          </Field>
          <Field label={t("settings.general.userName")}>
            <input
              value={draft.codex.userName}
              maxLength={80}
              placeholder={t("settings.general.userNamePlaceholder")}
              onChange={(event) =>
                onChange({ ...draft, codex: { ...draft.codex, userName: event.target.value } })
              }
            />
          </Field>
        </div>
      </section>
      <section className="settings-card">
        <h3>{t("settings.general.runtimeState")}</h3>
        <div className="settings-summary-grid">
          <Metric
            label={t("settings.general.harness")}
            value={draft.providers.harness.address || t("common.notConfigured")}
          />
          <Metric
            label={t("settings.general.individualProviders")}
            value={t("settings.general.enabledCount", { count: enabledProviders })}
          />
          <Metric
            label={t("settings.general.listening")}
            value={
              draft.voice.listeningEnabled ? t("settings.general.alwaysOn") : t("common.paused")
            }
          />
        </div>
      </section>
    </div>
  );
}
