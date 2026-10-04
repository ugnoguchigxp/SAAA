import type { RoleRoutingSettings } from "../../lib/roleRoutingTypes";
import type { CodexAgentSettings, ModelProvidersSettings } from "../../lib/contracts";

export function prepareRoutingActors(
  settings: RoleRoutingSettings,
  availableProviders: ModelProvidersSettings["providers"],
  codex: CodexAgentSettings,
): RoleRoutingSettings {
  const providerActors = availableProviders.map((provider) => ({
    id: `provider-${provider.id}`,
    label: provider.label,
    aliases: [],
    transport: "provider" as const,
    providerId: provider.id,
    model: null,
    location: provider.location,
    resourceGroup: provider.location === "local" ? "local-llm" : "cloud-llm",
    maxInputBytes: 65_536,
    capabilities: ["reason", "tools"],
  }));
  const codexActor =
    codex.enabled && codex.health === "ready"
      ? [
          {
            id: "codex-sol",
            label: "Codex Sol",
            aliases: ["sol"],
            transport: "codex_sdk" as const,
            providerId: null,
            model: "gpt-5.6-sol",
            location: "cloud" as const,
            resourceGroup: "codex-sdk",
            maxInputBytes: 65_536,
            capabilities: ["reason", "tools", "review"],
          },
        ]
      : [];
  const actors = [...providerActors, ...codexActor];
  const reasoner = settings.roles.reasoner ?? actors[0]?.id ?? null;
  return {
    ...settings,
    actors,
    roles: { ...settings.roles, reasoner },
    recipes: settings.recipes.length
      ? settings.recipes
      : reasoner
        ? [{ id: "respond-reasoner", action: "respond", roles: ["reasoner"], enabled: true }]
        : [],
  };
}
