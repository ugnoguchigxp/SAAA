import { roleLearningDefaults } from "./roleLearningDefaults";
import type { RoleRoutingSettings } from "../../lib/roleRoutingTypes";

export function defaultRoleRoutingSettings(): RoleRoutingSettings {
  return {
    schemaVersion: 1,
    enabled: true,
    actors: [
      {
        id: "larm-reasoner",
        label: "LARM 思考（llm）",
        aliases: [],
        transport: "provider",
        providerId: "lan-llm-dynamic",
        model: null,
        location: "local",
        resourceGroup: "larm-llm",
        maxInputBytes: 65536,
        larmProvider: "llm",
        capabilities: ["reason", "tools"],
      },
    ],
    roles: {
      frontend: null,
      reasoner: "larm-reasoner",
      advanced: null,
      reviewer: null,
      premium: null,
      toolSpecialist: null,
    },
    recipes: [
      {
        id: "00-butler-respond",
        action: "respond",
        roles: ["reasoner"],
        enabled: true,
      },
    ],
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
    ...roleLearningDefaults(),
  };
}
