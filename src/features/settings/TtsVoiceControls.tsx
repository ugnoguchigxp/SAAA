import { useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import type { CloudTtsProviderSettings } from "../../lib/settingsTypes";
import { loadTtsVoiceCatalog, type TtsVoiceCatalog } from "../../lib/runtime";
import { Field } from "./SettingsFields";

export function TtsVoiceControls({
  provider,
  onChange,
}: {
  provider: CloudTtsProviderSettings;
  onChange: (value: CloudTtsProviderSettings) => void;
}) {
  const { t } = useTranslation();
  const [catalog, setCatalog] = useState<TtsVoiceCatalog | null>(null);
  const [status, setStatus] = useState("");
  const requestId = useRef(0);
  const fingerprint = useRef("");
  fingerprint.current = `${provider.id}:${provider.endpoint}:${provider.model}`;
  const voicevox = provider.model === "voicevox-core";
  if (!voicevox) {
    return <p>{t("settings.providers.voicevoxHidden")}</p>;
  }
  const selected = catalog?.voices.find((voice) => voice.id === provider.voice);
  return (
    <>
      <button
        type="button"
        onClick={() => {
          const current = ++requestId.current;
          const started = fingerprint.current;
          setStatus(t("settings.providers.loadVoices"));
          void loadTtsVoiceCatalog({ source: "provider", providerId: provider.id })
            .then((next) => {
              if (current !== requestId.current || fingerprint.current !== started) return;
              setCatalog(next);
              setStatus("");
            })
            .catch(() => {
              if (current === requestId.current) setStatus(t("settings.providers.catalogFailed"));
            });
        }}
      >
        {t("settings.providers.loadVoices")}
      </button>
      <span aria-live="polite">{status}</span>
      <Field label={t("settings.providers.voice")}>
        <select
          value={provider.voice}
          onChange={(event) => {
            const voice = catalog?.voices.find((item) => item.id === event.target.value);
            onChange({
              ...provider,
              voice: event.target.value,
              style: voice?.defaultStyle,
            });
          }}
        >
          {catalog?.voices.some((voice) => voice.id === provider.voice) ? null : (
            <option value={provider.voice}>{provider.voice}</option>
          )}
          {catalog?.voices.map((voice) => (
            <option key={voice.id} value={voice.id}>
              {voice.displayName} ({voice.id}
              {voice.voicePresentation ? ` · ${voice.voicePresentation}` : ""})
            </option>
          ))}
        </select>
      </Field>
      <Field label={t("settings.providers.voiceStyle")}>
        <select
          value={provider.style ?? ""}
          onChange={(event) =>
            onChange({ ...provider, style: event.target.value || undefined })
          }
        >
          <option value="">{t("settings.providers.apiDefault")}</option>
          {selected?.styles.map((style) => (
            <option key={style.id} value={style.id}>
              {style.displayName}
            </option>
          ))}
          {provider.style && !selected?.styles.some((style) => style.id === provider.style) ? (
            <option value={provider.style}>{provider.style}</option>
          ) : null}
        </select>
      </Field>
      <Prosody
        label={t("settings.providers.speed")}
        min={0.5}
        max={2}
        step={0.05}
        value={provider.speed}
        onChange={(speed) => onChange({ ...provider, speed })}
      />
      <Prosody
        label={t("settings.providers.pitch")}
        min={-0.15}
        max={0.15}
        step={0.01}
        value={provider.pitchScale}
        onChange={(pitchScale) => onChange({ ...provider, pitchScale })}
      />
      <Prosody
        label={t("settings.providers.intonation")}
        min={0}
        max={2}
        step={0.05}
        value={provider.intonationScale}
        onChange={(intonationScale) => onChange({ ...provider, intonationScale })}
      />
      <p>{t("settings.providers.prosodyNote")}</p>
    </>
  );
}

function Prosody({
  label,
  min,
  max,
  step,
  value,
  onChange,
}: {
  label: string;
  min: number;
  max: number;
  step: number;
  value: number | undefined;
  onChange: (value: number | undefined) => void;
}) {
  const { t } = useTranslation();
  const shown = value ?? (min < 0 ? 0 : 1);
  return (
    <Field label={label}>
      <input
        type="range"
        min={min}
        max={max}
        step={step}
        value={shown}
        onChange={(event) => onChange(Number(event.target.value))}
      />
      <input
        type="number"
        min={min}
        max={max}
        step={step}
        value={value ?? ""}
        onChange={(event) => {
          if (event.target.value === "") {
            onChange(undefined);
            return;
          }
          const next = Number(event.target.value);
          if (Number.isFinite(next)) onChange(next);
        }}
      />
      <button type="button" onClick={() => onChange(undefined)}>
        {t("settings.providers.apiDefault")}
      </button>
    </Field>
  );
}
