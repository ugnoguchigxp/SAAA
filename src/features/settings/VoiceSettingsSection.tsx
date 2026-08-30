import { useEffect, useState } from "react";
import type { VoiceProfileSnapshot, VoiceSettings } from "../../lib/contracts";
import {
  enumerateAudioInputDevices,
  microphoneErrorMessage,
} from "../../lib/microphone";
import { ASR_LANGUAGES, type AsrLanguageCode } from "../../lib/asrLanguages";
import { Field, Metric } from "./SettingsFields";
import { VoiceProfileCard } from "./VoiceProfileCard";

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
            <p className="eyebrow">ALWAYS-ON VOICE</p>
            <h3>常時待ち受け</h3>
            <p className="settings-help">
              有効にするとSAAAの起動中はローカルVADで待ち受けます。Cloud
              選択したASRサービスへ送るのは、発話として確定した区間だけです。
            </p>
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
            label="Listening"
            value={voice.listeningEnabled ? "always on" : "paused"}
          />
          <Metric label="Detection" value="Local VAD" />
          <Metric label="Cloud upload" value="Finalized speech only" />
          <Metric label="Activation" value="automatic" />
        </div>
        <p className="settings-help">
          初回はmacOSのマイク許可が表示されます。Meeting・応答の読み上げ中は自動的に一時停止し、終了後に再開します。声の登録やサンプル再生は、Chatで常時待ち受けを一時停止してから実行してください。
        </p>
      </section>

      <section className="settings-card">
        <h3>Audio devices</h3>
        <div className="settings-form-grid">
          <Field label="Input device">
            <select
              value={voice.inputDeviceId}
              onChange={(event) =>
                onChange({ ...voice, inputDeviceId: event.target.value })
              }
            >
              <option value="default">System default</option>
              {missingDevice && (
                <option value={voice.inputDeviceId}>
                  Previously selected (unavailable)
                </option>
              )}
              {devices.map((device, index) => (
                <option key={device.deviceId} value={device.deviceId}>
                  {device.label || `Microphone ${index + 1}`}
                </option>
              ))}
            </select>
          </Field>
          <Field label="Output device">
            <select value={voice.outputDeviceId} disabled>
              <option value="default">System default</option>
            </select>
          </Field>
        </div>
        {deviceError && (
          <p className="provider-test-result error">{deviceError}</p>
        )}
        <p className="settings-help">音声の再生先はmacOSのシステム出力設定に従います。</p>
      </section>

      <section className="settings-card">
        <h3>Speech detection</h3>
        <div className="settings-form-grid">
          <Field label="VAD sensitivity">
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
              <option value="low">Low · noisy room</option>
              <option value="medium">Medium (recommended)</option>
              <option value="high">High · quiet speech</option>
            </select>
          </Field>
          <Field label="Silence timeout (ms)">
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
          <strong>使用する言語</strong>
          <p className="settings-help">
            ASRは言語を自動判定します。ここに登録していない言語、または判定できない音声は会話や議事録へ送りません。
          </p>
          <div
            className="language-options"
            role="group"
            aria-label="使用する言語"
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
                {language.label}
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
          応答を音声で再生する
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
