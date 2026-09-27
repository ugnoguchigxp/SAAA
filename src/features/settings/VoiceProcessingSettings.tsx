import { useTranslation } from "react-i18next";
import type { VoiceSettings } from "../../lib/contracts";
import { Field } from "./SettingsFields";

export function VoiceProcessingSettings({
  voice,
  onChange,
}: {
  voice: VoiceSettings;
  onChange: (value: VoiceSettings) => void;
}) {
  const { t } = useTranslation();
  return (
    <section className="settings-card">
      <h3>{t("voice.processingTitle")}</h3>
      <label className="check-row">
        <input
          type="checkbox"
          checked={voice.aecEnabled}
          onChange={(event) => onChange({ ...voice, aecEnabled: event.target.checked })}
        />
        {t("voice.aecEnabled")}
      </label>
      <p className="settings-help">{t("voice.aecHelp")}</p>
      <p className="settings-help">{t("voice.aecFallback")}</p>
      <div className="settings-form-grid">
        <Field label={t("voice.duckingLevel")}>
          <select
            value={voice.otherAudioDucking}
            onChange={(event) =>
              onChange({
                ...voice,
                otherAudioDucking: event.target.value as VoiceSettings["otherAudioDucking"],
              })
            }
          >
            <option value="min">{t("voice.duckingMin")}</option>
            <option value="default">{t("voice.duckingDefault")}</option>
            <option value="mid">{t("voice.duckingMid")}</option>
            <option value="max">{t("voice.duckingMax")}</option>
          </select>
        </Field>
      </div>
      <label className="check-row">
        <input
          type="checkbox"
          checked={voice.vpioOnBluetooth}
          onChange={(event) => onChange({ ...voice, vpioOnBluetooth: event.target.checked })}
        />
        {t("voice.vpioOnBluetooth")}
      </label>
      <label className="check-row">
        <input
          type="checkbox"
          checked={voice.bargeInEnabled}
          onChange={(event) => onChange({ ...voice, bargeInEnabled: event.target.checked })}
        />
        {t("voice.bargeInEnabled")}
      </label>
    </section>
  );
}
