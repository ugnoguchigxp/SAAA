import {
  findSettingsDocument,
  isCodexAgentSettings,
  isModelProvidersSettings,
  isRoutingSettings,
  isSecuritySettings,
  isSituationSettings,
  isVoiceSettings,
  type CodexAgentSettings,
  type ModelProvidersSettings,
  type RoutingSettings,
  type SecuritySettings,
  type SettingsDocument,
  type SettingsNamespace,
  type SituationSettings,
  type VoiceSettings,
} from "../../lib/contracts";

export type SettingsDraft = {
  providers: ModelProvidersSettings;
  codex: CodexAgentSettings;
  routing: RoutingSettings;
  voice: VoiceSettings;
  security: SecuritySettings;
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
  const situation = find("situation.runtime", "default");
  return {
    providers: model && isModelProvidersSettings(model) ? model : fallback.providers,
    codex: codex && isCodexAgentSettings(codex) ? codex : fallback.codex,
    routing: routing && isRoutingSettings(routing) ? routing : fallback.routing,
    voice: voice && isVoiceSettings(voice) ? voice : fallback.voice,
    security: security && isSecuritySettings(security) ? security : fallback.security,
    situation: situation && isSituationSettings(situation) ? situation : fallback.situation,
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
    document("situation.runtime", "default", draft.situation),
  ];
}

function document(namespace: SettingsNamespace, key: "default" | "codex-sdk", valueJson: Record<string, unknown>): Omit<SettingsDocument, "updatedAt"> {
  return { namespace, key, schemaVersion: 11, valueJson };
}
