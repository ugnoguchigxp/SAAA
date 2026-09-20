export type RoleRoutingActor = {
  id: string;
  label: string;
  aliases: string[];
  transport: "provider" | "codex_sdk";
  providerId: string | null;
  model: string | null;
  location: "local" | "cloud";
  resourceGroup: string;
  maxInputBytes: number;
  capabilities: string[];
};

export type RoleRoutingRecipe = {
  id: string;
  action:
    | "respond"
    | "explain"
    | "clarify"
    | "reconsider_same"
    | "reconsider_other"
    | "review_other"
    | "revise"
    | "propose_upgrade"
    | "finalize"
    | "cancel";
  roles: string[];
  enabled: boolean;
};

export type RoleRoutingLimits = {
  maxReasoningSteps: number;
  maxToolCalls: number;
  rootTimeoutMs: number;
  stepTimeoutMs: number;
  frontendTimeoutMs: number;
  classificationTimeoutMs: number;
  maxQueuedInputs: number;
  maxReviewRounds: number;
  maxAutomaticSwitches: number;
  maxEstimatedCostMicros: number | null;
};

export type RoleRoutingSpeech = {
  mode: "author_verbatim";
  ackDelayMs: number;
  maxAckChars: number;
  progressMinIntervalMs: number;
  maxProgressPerRoot: number;
};

export type RoleRoutingSelection = {
  mode: "rules" | "shadow";
  shadowArtifactId: string | null;
  classificationMinConfidence: number;
  weights: { quality: number; latency: number; cost: number };
  switchMargin: number;
};

export type RoleRoutingLearning = {
  enabled: boolean;
  localStart: string;
  localEnd: string;
  idleSeconds: number;
  maxRunSeconds: number;
  batchSize: number;
  allowLocalLabeler: boolean;
};
export type AdaptiveImprovementSettings = {
  enabled: boolean;
  providerRecipe: boolean;
  tool: boolean;
  plan: boolean;
  notification: boolean;
};

export type RoleRoutingSettings = {
  schemaVersion: 1;
  enabled: boolean;
  actors: RoleRoutingActor[];
  roles: {
    frontend: string | null;
    reasoner: string | null;
    advanced: string | null;
    reviewer: string | null;
    premium: string | null;
    toolSpecialist: string | null;
  };
  recipes: RoleRoutingRecipe[];
  limits: RoleRoutingLimits;
  speech: RoleRoutingSpeech;
  selection: RoleRoutingSelection;
  premiumApproval: "per_request" | "never";
  learning: RoleRoutingLearning;
  adaptiveImprovement: AdaptiveImprovementSettings;
};
