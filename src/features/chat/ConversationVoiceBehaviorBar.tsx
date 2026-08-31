import { useTranslation } from "react-i18next";

import type { ConversationVoicePolicySnapshot } from "../../lib/contracts";
import "./ConversationVoiceBehaviorBar.css";

export function ConversationVoiceBehaviorBar({
  policy,
  disabled,
  onOpenSettings,
  onSetSpeechOutput,
  onSetListeningPace,
  onReset,
}: {
  policy: ConversationVoicePolicySnapshot;
  disabled: boolean;
  onOpenSettings: () => void;
  onSetSpeechOutput: (value: "inherit" | "muted") => void;
  onSetListeningPace: (value: "inherit" | "quick" | "balanced" | "patient") => void;
  onReset: () => void;
}) {
  const { t } = useTranslation();
  return <div className="voice-behavior-bar" aria-live="polite" aria-busy={disabled}>
    <span className={`voice-behavior-state ${policy.effectiveSpeechOutput}`}>{policy.speechReasonCode === "global_opt_out" ? t("chat.voiceGlobalQuiet") : policy.effectiveSpeechOutput === "silent" ? t("chat.voiceQuiet") : t("chat.voiceOn")}</span>
    {policy.speechReasonCode === "global_opt_out" ? <button type="button" className="text-button" onClick={onOpenSettings} disabled={disabled}>{t("chat.voiceOpenSettings")}</button> : policy.speechOutput === "muted" ? <button type="button" className="text-button" onClick={() => onSetSpeechOutput("inherit")} disabled={disabled}>{t("chat.voiceResume")}</button> : <button type="button" className="text-button" onClick={() => onSetSpeechOutput("muted")} disabled={disabled}>{t("chat.voiceQuietAction")}</button>}
    <label className="voice-pace-control">{t("chat.listeningPace")}<select value={policy.listeningPace} onChange={(event) => onSetListeningPace(event.currentTarget.value as "inherit" | "quick" | "balanced" | "patient")} disabled={disabled}><option value="inherit">{t("chat.paceDefault")}</option><option value="quick">{t("chat.paceQuick")}</option><option value="balanced">{t("chat.paceBalanced")}</option><option value="patient">{t("chat.pacePatient")}</option></select></label>
    <span className="voice-pace-effect">{t("chat.paceTakesEffectNextTurn")}</span>
    {(policy.speechOutput !== "inherit" || policy.listeningPace !== "inherit") && <button type="button" className="text-button" onClick={onReset} disabled={disabled}>{t("chat.voiceReset")}</button>}
  </div>;
}
