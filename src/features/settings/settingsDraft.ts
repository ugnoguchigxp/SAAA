import {
  findSettingsDocument,
  type CodexAgentSettings,
  type ModelProvidersSettings,
  type RoutingSettings,
  type RegionalPreferencesSettings,
  type SecuritySettings,
  type SettingsDocument,
  type SettingsNamespace,
  type SituationSettings,
  type VoiceSettings,
} from "../../lib/contracts";
import {
  codexAgentSettingsSchema,
  modelProvidersSettingsSchema,
  regionalPreferencesSchema,
  routingSettingsSchema,
  securitySettingsSchema,
  situationSettingsSchema,
  voiceSettingsSchema,
} from "../../lib/schemas";

export type SettingsDraft = {
  providers: ModelProvidersSettings;
  codex: CodexAgentSettings;
  routing: RoutingSettings;
  voice: VoiceSettings;
  security: SecuritySettings;
  regional: RegionalPreferencesSettings;
  situation: SituationSettings;
};

export function draftFromDocuments(documents: SettingsDocument[], fallback: SettingsDraft): SettingsDraft {
  const find = (namespace: SettingsNamespace, key: "default" | "codex-sdk") =>
    findSettingsDocument(documents, namespace, key)?.valueJson;
  const model = find("providers.model", "default");
  const codex = find("providers.agent", "codex-sdk");
  const routing = find("routing.tasks", "default");
  const voice = find("voice.runtime", "default");
  const security = find("security.runtime", "default");
  const regional = find("ui.preferences", "default");
  const situation = find("situation.runtime", "default");
  const parsedModel = modelProvidersSettingsSchema.safeParse(model);
  const parsedCodex = codexAgentSettingsSchema.safeParse(codex);
  const parsedRouting = routingSettingsSchema.safeParse(routing);
  const parsedVoice = voiceSettingsSchema.safeParse(voice);
  const parsedSecurity = securitySettingsSchema.safeParse(security);
  const parsedRegional = regionalPreferencesSchema.safeParse(regional);
  const parsedSituation = situationSettingsSchema.safeParse(situation);
  return {
    providers: parsedModel.success ? parsedModel.data : fallback.providers,
    codex: parsedCodex.success ? parsedCodex.data : fallback.codex,
    routing: parsedRouting.success ? parsedRouting.data : fallback.routing,
    voice: parsedVoice.success ? parsedVoice.data : fallback.voice,
    security: parsedSecurity.success ? parsedSecurity.data : fallback.security,
    regional: parsedRegional.success ? parsedRegional.data : fallback.regional,
    situation: parsedSituation.success ? parsedSituation.data : fallback.situation,
  };
}

export function documentsFromDraft(draft: SettingsDraft): Array<Omit<SettingsDocument, "updatedAt">> {
  return [
    document("providers.model", "default", draft.providers),
    document("providers.agent", "codex-sdk", {
      ...draft.codex,
      agentName: draft.codex.agentName.trim(),
      userName: draft.codex.userName.trim(),
    }),
    document("routing.tasks", "default", draft.routing),
    document("voice.runtime", "default", draft.voice),
    document("security.runtime", "default", draft.security),
    document("ui.preferences", "default", draft.regional),
    document("situation.runtime", "default", draft.situation),
  ];
}

export function reconcileSavedDraft(
  current: SettingsDraft,
  submittedFingerprint: string,
  saved: SettingsDocument[],
): SettingsDraft {
  return JSON.stringify(current) === submittedFingerprint
    ? draftFromDocuments(saved, current)
    : current;
}

export function credentialCleanupProviderIds(source: SettingsDraft, draft: SettingsDraft): string[] {
  const nextProviders = new Map(draft.providers.providers.map((provider) => [provider.id, provider]));
  return source.providers.providers.flatMap((provider) => {
    if (!("authentication" in provider)) return [];
    const next = nextProviders.get(provider.id);
    return !next || !("authentication" in next) || next.authentication !== "api-key"
      ? [provider.id]
      : [];
  });
}

function document(namespace: SettingsNamespace, key: "default" | "codex-sdk", valueJson: Record<string, unknown>): Omit<SettingsDocument, "updatedAt"> {
  return { namespace, key, schemaVersion: 14, valueJson };
}
