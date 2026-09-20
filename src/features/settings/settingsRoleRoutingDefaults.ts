import type { RoleRoutingSettings } from "../../lib/roleRoutingTypes";

export function defaultRoleRoutingSettings(): RoleRoutingSettings {
  return {
    schemaVersion: 1,
    enabled: false,
    actors: [],
    roles: {
      frontend: null,
      reasoner: null,
      advanced: null,
      reviewer: null,
      premium: null,
      toolSpecialist: null,
    },
    recipes: [],
    limits: {
      maxReasoningSteps: 4,
      maxToolCalls: 32,
      rootTimeoutMs: 180_000,
      stepTimeoutMs: 60_000,
      frontendTimeoutMs: 1_200,
      classificationTimeoutMs: 1_500,
      maxQueuedInputs: 4,
      maxReviewRounds: 1,
      maxAutomaticSwitches: 2,
      maxEstimatedCostMicros: null,
    },
    speech: {
      mode: "author_verbatim",
      ackDelayMs: 250,
      maxAckChars: 80,
      progressMinIntervalMs: 15_000,
      maxProgressPerRoot: 2,
    },
    selection: {
      mode: "rules",
      shadowArtifactId: null,
      classificationMinConfidence: 0.85,
      weights: { quality: 0.6, latency: 0.25, cost: 0.15 },
      switchMargin: 0.15,
    },
    premiumApproval: "per_request",
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
