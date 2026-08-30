import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import type { VoiceProfileSnapshot, VoiceSettings } from "../../lib/contracts";
import {
  enumerateAudioInputDevices,
  microphoneErrorMessage,
} from "../../lib/microphone";
import { ASR_LANGUAGES, type AsrLanguageCode } from "../../lib/asrLanguages";
import { Field, Metric } from "./SettingsFields";
import { VoiceProfileCard } from "./VoiceProfileCard";
import { localizeUiMessage } from "../../i18n/presentation";

export function VoiceSettingsSection({
  voice,
  profile,
  enrollmentBlocked,
  onChange,
  onProfileChanged,
}: {
  voice: VoiceSettings;
  profile: VoiceProfileSnapshot;
  enrollmentBlocked: boolean;
  onChange: (value: VoiceSettings) => void;
  onProfileChanged: (profile: VoiceProfileSnapshot) => void;
}) {
  const { t } = useTranslation();
  const [devices, setDevices] = useState<MediaDeviceInfo[]>([]);
  const [deviceError, setDeviceError] = useState<string | null>(null);
  useEffect(() => {
    let active = true;
    void enumerateAudioInputDevices()
      .then((available) => {
        if (active) setDevices(available);
      })
      .catch((cause) => {
        if (active) setDeviceError(microphoneErrorMessage(cause));
      });
    return () => {
      active = false;
    };
  }, []);
  const missingDevice =
    voice.inputDeviceId !== "default" &&
    !devices.some((device) => device.deviceId === voice.inputDeviceId);

  return (
    <div className="settings-stack">
      <section className="settings-card">
        <div className="card-title-row">
          <div>
            <p className="eyebrow">{t("voice.eyebrow")}</p>
            <h3>{t("voice.alwaysOnTitle")}</h3>
            <p className="settings-help">{t("voice.alwaysOnDescription")}</p>
          </div>
          <label className="toggle">
            <input
              type="checkbox"
              checked={voice.listeningEnabled}
              onChange={(event) =>
                onChange({ ...voice, listeningEnabled: event.target.checked })
              }
            />
            <span />
          </label>
        </div>
        <div className="settings-summary-grid">
          <Metric
            label={t("voice.listening")}
            value={voice.listeningEnabled ? t("settings.general.alwaysOn") : t("common.paused")}
          />
          <Metric label={t("voice.detection")} value={t("voice.localVad")} />
          <Metric label={t("voice.cloudUpload")} value={t("voice.continuousChunks")} />
          <Metric label={t("voice.activation")} value={t("voice.automatic")} />
        </div>
        <p className="settings-help">{t("voice.permissionHelp")}</p>
      </section>

      <section className="settings-card">
        <h3>{t("voice.audioDevices")}</h3>
        <div className="settings-form-grid">
          <Field label={t("voice.inputDevice")}>
            <select
              value={voice.inputDeviceId}
              onChange={(event) =>
                onChange({ ...voice, inputDeviceId: event.target.value })
              }
            >
              <option value="default">{t("common.systemDefault")}</option>
              {missingDevice && (
                <option value={voice.inputDeviceId}>
                  {t("voice.unavailableDevice")}
                </option>
              )}
              {devices.map((device, index) => (
                <option key={device.deviceId} value={device.deviceId}>
                  {device.label || t("voice.microphoneNumber", { number: index + 1 })}
                </option>
              ))}
            </select>
          </Field>
          <Field label={t("voice.outputDevice")}>
            <select value={voice.outputDeviceId} disabled>
              <option value="default">{t("common.systemDefault")}</option>
            </select>
          </Field>
        </div>
        {deviceError && (
          <p className="provider-test-result error">{localizeUiMessage(t, deviceError, "voice")}</p>
        )}
        <p className="settings-help">{t("voice.outputHelp")}</p>
      </section>

      <section className="settings-card">
        <h3>{t("voice.detectionTitle")}</h3>
        <div className="settings-form-grid">
          <Field label={t("voice.sensitivity")}>
            <select
              value={voice.vadSensitivity}
              onChange={(event) =>
                onChange({
                  ...voice,
                  vadSensitivity: event.target
                    .value as VoiceSettings["vadSensitivity"],
                })
              }
            >
              <option value="low">{t("voice.lowSensitivity")}</option>
              <option value="medium">{t("voice.mediumSensitivity")}</option>
              <option value="high">{t("voice.highSensitivity")}</option>
            </select>
          </Field>
          <Field label={t("voice.silenceTimeout")}>
            <input
              type="number"
              min="800"
              max="3000"
              step="100"
              value={voice.silenceTimeoutMs}
              onChange={(event) =>
                onChange({
                  ...voice,
                  silenceTimeoutMs: Math.max(
                    800,
                    Math.min(3000, Number(event.target.value) || 1500),
                  ),
                })
              }
            />
          </Field>
        </div>
        <div>
          <strong>{t("voice.languages")}</strong>
          <p className="settings-help">{t("voice.languagesHelp")}</p>
          <div
            className="language-options"
            role="group"
            aria-label={t("voice.languagesAria")}
          >
            {ASR_LANGUAGES.map((language) => (
              <label className="check-row" key={language.code}>
                <input
                  type="checkbox"
                  checked={voice.allowedLanguages.includes(language.code)}
                  disabled={
                    voice.allowedLanguages.length === 1 &&
                    voice.allowedLanguages[0] === language.code
                  }
                  onChange={(event) =>
                    onChange({
                      ...voice,
                      allowedLanguages: updateLanguages(
                        voice.allowedLanguages,
                        language.code,
                        event.target.checked,
                      ),
                    })
                  }
                />
                {t(`asrLanguages.${language.code}`, { defaultValue: language.label })}
              </label>
            ))}
          </div>
        </div>
        <label className="check-row">
          <input
            type="checkbox"
            checked={voice.autoSpeak}
            onChange={(event) =>
              onChange({ ...voice, autoSpeak: event.target.checked })
            }
          />
          {t("voice.autoSpeak")}
        </label>
      </section>

      <VoiceProfileCard
        voice={voice}
        profile={profile}
        blocked={enrollmentBlocked}
        onChanged={onProfileChanged}
      />
    </div>
  );
}

function updateLanguages(
  current: AsrLanguageCode[],
  code: AsrLanguageCode,
  checked: boolean,
): AsrLanguageCode[] {
  if (checked) return current.includes(code) ? current : [...current, code];
  return current.length === 1
    ? current
    : current.filter((language) => language !== code);
}
