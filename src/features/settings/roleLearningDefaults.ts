import type { RoleRoutingSettings } from "../../lib/roleRoutingTypes";

export function roleLearningDefaults(): Pick<
  RoleRoutingSettings,
  "learning" | "adaptiveImprovement"
> {
  return {
    learning: {
      enabled: false,
      localStart: "02:00",
      localEnd: "05:00",
      idleSeconds: 300,
      maxRunSeconds: 600,
      batchSize: 100,
      allowLocalLabeler: false,
    },
    adaptiveImprovement: {
      enabled: false,
      providerRecipe: false,
      tool: false,
      plan: false,
      notification: false,
    },
  };
}
