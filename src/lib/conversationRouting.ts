import {
  findSettingsDocument,
  isModelProvidersSettings,
  isRoutingSettings,
  type AppSnapshot,
  type SettingsDocument,
} from "./contracts";

export function findPrimaryRoute(documents: SettingsDocument[]): string {
  const routing = findSettingsDocument(documents, "routing.tasks", "default");
  if (routing && typeof routing.valueJson.conversationRespond === "object" && routing.valueJson.conversationRespond !== null) {
    const value = routing.valueJson.conversationRespond as Record<string, unknown>;
    if (typeof value.primaryProviderId === "string") return value.primaryProviderId;
  }
  return "gnosis-qwen";
}

export function updateConversationTimestamp(snapshot: AppSnapshot, conversationId: string, title: string): AppSnapshot {
  const conversation = snapshot.conversations.find((item) => item.id === conversationId);
  if (!conversation) return snapshot;
  return { ...snapshot, conversations: [{ ...conversation, title: conversation.title ?? title.slice(0, 60), updatedAt: "pending" }, ...snapshot.conversations.filter((item) => item.id !== conversationId)] };
}

export function resolveModelProviderStatus(snapshot: AppSnapshot): { ready: boolean; label: string } {
  const document = findSettingsDocument(snapshot.settings, "providers.model", "default");
  if (!document || !isModelProvidersSettings(document.valueJson)) {
    return { ready: false, label: "モデル未選択" };
  }
  const providers = document.valueJson.providers;
  const primaryId = findPrimaryRoute(snapshot.settings);
  const primary = providers.find((provider) => provider.id === primaryId);
  if (!primary) return { ready: false, label: "モデル未選択" };
  if (primary?.kind === "larm" && snapshot.larmRuntime.state !== "ready") {
    const routing = findSettingsDocument(snapshot.settings, "routing.tasks", "default");
    const fallbackIds = routing && isRoutingSettings(routing.valueJson)
      ? routing.valueJson.conversationRespond.fallbackProviderIds
      : [];
    const fallback = fallbackIds
      .map((id) => providers.find((provider) => provider.id === id))
      .find((provider) => provider?.enabled && provider.kind !== "larm");
    if (fallback?.kind === "openai-compatible") {
      return {
        ready: Boolean(fallback.endpoint.trim() && fallback.model.trim()),
        label: fallback.label || fallback.id,
      };
    }
    if (fallback?.kind === "gnosis") {
      return {
        ready: Boolean(fallback.host.trim()),
        label: fallback.label || fallback.id,
      };
    }
    return { ready: false, label: primary.label || primary.id };
  }
  return {
    ready: primary.kind === "larm"
      ? primary.enabled && snapshot.larmRuntime.state === "ready"
      : primary.kind === "gnosis"
        ? primary.enabled && Boolean(primary.host.trim())
        : primary.enabled && Boolean(primary.endpoint.trim() && primary.model.trim()),
    label: primary.label || primary.id,
  };
}
