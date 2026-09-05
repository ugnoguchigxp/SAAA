import {
  findSettingsDocument,
  type AppSnapshot,
  type EffectiveRouteSnapshot,
  type SettingsDocument,
} from "./contracts";
import { modelProvidersSettingsSchema } from "./providerSchemas";

export function findPrimaryRoute(documents: SettingsDocument[]): string {
  const routing = findSettingsDocument(documents, "routing.tasks", "default");
  if (routing && typeof routing.valueJson.conversationRespond === "object" && routing.valueJson.conversationRespond !== null) {
    const value = routing.valueJson.conversationRespond as Record<string, unknown>;
    if (typeof value.primaryProviderId === "string") return value.primaryProviderId;
  }
  return "lan-llm-dynamic";
}

export function updateConversationTimestamp(snapshot: AppSnapshot, conversationId: string, title: string): AppSnapshot {
  const conversation = snapshot.conversations.find((item) => item.id === conversationId);
  if (!conversation) return snapshot;
  return { ...snapshot, conversations: [{ ...conversation, title: conversation.title ?? title.slice(0, 60), updatedAt: "pending" }, ...snapshot.conversations.filter((item) => item.id !== conversationId)] };
}

export function resolveModelProviderStatus(snapshot: AppSnapshot): EffectiveRouteSnapshot & { ready: boolean } {
  return {
    ...snapshot.effectiveRoute,
    ready: snapshot.effectiveRoute.state === "active" || snapshot.effectiveRoute.state === "ready",
  };
}

export function updateEffectiveRoute(
  snapshot: AppSnapshot,
  providerId: string,
  state: EffectiveRouteSnapshot["state"],
  options: { fallbackUsed?: boolean; reasonCode: string },
): AppSnapshot {
  const document = findSettingsDocument(snapshot.settings, "providers.model", "default");
  const parsed = modelProvidersSettingsSchema.safeParse(document?.valueJson);
  const provider = parsed.success
    ? parsed.data.providers.find((candidate) => candidate.id === providerId)
    : undefined;
  const routeFallback = providerId !== findPrimaryRoute(snapshot.settings);
  return {
    ...snapshot,
    effectiveRoute: {
      providerId,
      label: provider?.label || providerId,
      location: provider?.location ?? null,
      state,
      fallbackUsed: Boolean(options.fallbackUsed) || routeFallback,
      reasonCode: routeFallback ? "fallback-route" : options.reasonCode,
      updatedAt: new Date().toISOString(),
    },
  };
}
