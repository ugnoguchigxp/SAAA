import { useTranslation } from "react-i18next";
import { AppIcon } from "../../components/AppIcon";
import type { ConversationVoicePolicySnapshot } from "../../lib/contracts";
import { ConversationVoiceBehaviorBar } from "./ConversationVoiceBehaviorBar";
import { UiControls } from "./ui/UiControls";

export function ConversationBehaviorMenu({
  conversationId,
  policy,
  disabled,
  onOpenSettings,
  onOpenDiagnosis,
  onSetSpeechOutput,
  onSetListeningPace,
  onReset,
}: {
  conversationId?: string;
  policy: ConversationVoicePolicySnapshot;
  disabled: boolean;
  onOpenSettings: () => void;
  onOpenDiagnosis: () => void;
  onSetSpeechOutput: (value: "inherit" | "muted") => void;
  onSetListeningPace: (value: "inherit" | "quick" | "balanced" | "patient") => void;
  onReset: () => void;
}) {
  const { t } = useTranslation();
  return (
    <details className="conversation-behavior-popover">
      <summary aria-label={t("chat.voiceBehaviorTitle")} title={t("chat.voiceBehaviorTitle")}>
        <AppIcon name="settings" />
      </summary>
      <div className="conversation-behavior-popover-panel">
        <UiControls conversationId={conversationId} />
        <ConversationVoiceBehaviorBar
          policy={policy}
          disabled={disabled}
          onOpenSettings={onOpenSettings}
          onSetSpeechOutput={onSetSpeechOutput}
          onSetListeningPace={onSetListeningPace}
          onReset={onReset}
        />
        <button type="button" onClick={onOpenDiagnosis}>
          {t("chat.diagnosis.open")}
        </button>
      </div>
    </details>
  );
}
