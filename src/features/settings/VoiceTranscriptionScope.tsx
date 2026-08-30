import { useTranslation } from "react-i18next";

export function VoiceTranscriptionScope({
  filterEnabled,
  disabled,
  canEnableFilter,
  onChange,
}: {
  filterEnabled: boolean;
  disabled: boolean;
  canEnableFilter: boolean;
  onChange: (enabled: boolean) => void;
}) {
  const { t } = useTranslation();
  return <fieldset className="language-options voice-transcription-scope" aria-label={t("voice.profile.transcriptionScope")}>
    <legend><strong>{t("voice.profile.transcriptionScope")}</strong></legend>
    <label className="check-row">
      <input type="radio" name="voice-transcription-scope" checked={!filterEnabled} disabled={disabled} onChange={() => onChange(false)} />
      {t("voice.profile.allVoices")}
    </label>
    <label className="check-row">
      <input type="radio" name="voice-transcription-scope" checked={filterEnabled} disabled={disabled || !canEnableFilter} onChange={() => onChange(true)} />
      {t("voice.profile.onlyMyVoice")}
    </label>
  </fieldset>;
}
