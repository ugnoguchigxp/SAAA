import { useTranslation } from "react-i18next";
import { findSettingsDocument, type AppSnapshot } from "../../lib/contracts";
import {
  modelProvidersSettingsSchema,
  routingSettingsSchema,
  voiceSettingsSchema,
} from "../../lib/schemas";

export type SetupStep = "provider" | "connection" | "asr" | "speaker" | "ready";
export function nextSetupStep(snapshot: AppSnapshot): SetupStep {
  const read = (namespace: Parameters<typeof findSettingsDocument>[1]) =>
    findSettingsDocument(snapshot.settings, namespace, "default")?.valueJson;
  const providers = modelProvidersSettingsSchema.safeParse(read("providers.model"));
  const routing = routingSettingsSchema.safeParse(read("routing.tasks"));
  if (
    !providers.success ||
    !routing.success ||
    snapshot.effectiveRoute.reasonCode === "snapshot-loading"
  )
    return "provider";
  if (snapshot.effectiveRoute.state !== "ready" && snapshot.effectiveRoute.state !== "active")
    return "connection";
  const voice = voiceSettingsSchema.safeParse(read("voice.runtime"));
  if (!voice.success || !voice.data.listeningEnabled) return "ready";
  const route = routing.data.voiceTranscribe;
  if (
    route.source === "provider" &&
    !providers.data.providers.some(
      (provider) =>
        provider.id === route.providerId && provider.enabled && provider.kind === "cloud-asr",
    )
  )
    return "asr";
  if (route.source === "harness" && !providers.data.harness.address.trim()) return "asr";
  if (snapshot.voiceProfile.filterEnabled && snapshot.voiceProfile.status !== "ready")
    return "speaker";
  return "ready";
}
export function SetupChecklist({
  snapshot,
  onOpenSettings,
}: {
  snapshot: AppSnapshot;
  onOpenSettings: () => void;
}) {
  const { t } = useTranslation();
  const step = nextSetupStep(snapshot);
  return (
    <div className="setup-checklist" role="status">
      <p>{t(`chat.setup.${step}`)}</p>
      {step !== "ready" && (
        <button type="button" onClick={onOpenSettings}>
          {t("chat.setup.openSettings")}
        </button>
      )}
    </div>
  );
}
