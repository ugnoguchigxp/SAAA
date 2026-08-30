import { z } from "zod";
import { ASR_LANGUAGE_CODES } from "./asrLanguages";
import { MAX_CONVERSATION_TIMEOUT_MS, MIN_CONVERSATION_TIMEOUT_MS } from "./conversationTimeout";
import { runtimeFailureCodes } from "./generated/runtimeEvent";
import { modelProvidersSettingsSchema, providerIdSchema } from "./providerSchemas";
import { CURRENCY_CODES, DISPLAY_LANGUAGE_PREFERENCES, isSupportedTimeZone, LENGTH_UNIT_SYSTEMS, WEIGHT_UNITS } from "./regionalPreferences";
export { modelProvidersSettingsSchema } from "./providerSchemas";

export const DYNAMIC_LAN_MAX_REQUEST_TIMEOUT_MS = 269_999;

export const runtimeFailureCodeSchema = z.enum(runtimeFailureCodes);

export const signalHealthSchema = z.enum([
  "ready",
  "disabled",
  "permission-denied",
  "unsupported",
  "degraded",
]);

export const inputActivitySignalSchema = z.object({
  state: z.enum(["active", "recent", "idle", "unknown"]),
  health: signalHealthSchema,
}).strict();

export const calibrationParametersSchema = z.object({
  classificationMinConfidence: z.number().int().min(50).max(95),
  lowConfidenceMax: z.number().int().min(0).max(60),
  enterSampleCount: z.number().int().min(1).max(10),
  exitSampleCount: z.number().int().min(1).max(20),
  cooldownMs: z.number().int().min(0).max(60_000),
  inputActiveMaxMs: z.number().int().min(5_000).max(120_000),
  inputRecentMaxMs: z.number().int().min(60_000).max(1_800_000),
}).strict().refine(
  (value) => value.lowConfidenceMax < value.classificationMinConfidence,
  { message: "Low-confidence maximum must be lower than classification confidence" },
).refine(
  (value) => value.inputActiveMaxMs < value.inputRecentMaxMs,
  { message: "Input active boundary must be lower than input recent boundary" },
);

export const codexAgentSettingsSchema = z.object({
  agentName: z.string().trim().min(1).max(80).refine(
    (value) => ![...value].some((character) => /[\u0000-\u001F\u007F]/.test(character)),
    "Agent name must not contain control characters",
  ),
  userName: z.string().trim().max(80).refine(
    (value) => ![...value].some((character) => /[\u0000-\u001F\u007F]/.test(character)),
    "User name must not contain control characters",
  ),
  enabled: z.boolean(),
  provider: z.literal("codex-sdk"),
  model: z.string().trim().max(160),
  runtimeMode: z.enum(["pending-compatibility-check", "bun", "node-sidecar", "app-server"]),
  health: z.enum(["unchecked", "ready", "unavailable"]),
  sandboxMode: z.literal("read-only"),
  approvalPolicy: z.literal("never"),
  networkEnabled: z.literal(false),
  webSearchEnabled: z.literal(false),
  workspacePolicy: z.literal("select-per-conversation"),
}).strict();

export const routingSettingsSchema = z.object({
  conversationRespond: z.object({
    source: z.enum(["harness", "provider"]),
    primaryProviderId: providerIdSchema.nullable(),
    fallbackProviderIds: z.array(providerIdSchema).max(20),
    timeoutMs: z.number().int().min(MIN_CONVERSATION_TIMEOUT_MS).max(MAX_CONVERSATION_TIMEOUT_MS),
  }).strict().superRefine((route, context) => {
    if (route.source === "provider" && !route.primaryProviderId) context.addIssue({ code: "custom", message: "Individual LLM source requires a provider", path: ["primaryProviderId"] });
    if (route.source === "harness" && route.primaryProviderId !== null) context.addIssue({ code: "custom", message: "Harness source must not reference an individual provider", path: ["primaryProviderId"] });
  }),
  voiceTranscribe: z.object({
    source: z.enum(["harness", "provider"]),
    providerId: providerIdSchema.nullable(),
    timeoutMs: z.number().int().min(1_000).max(300_000),
  }).strict().superRefine((route, context) => {
    if (route.source === "provider" && !route.providerId) context.addIssue({ code: "custom", message: "Individual ASR source requires a provider", path: ["providerId"] });
    if (route.source === "harness" && route.providerId !== null) context.addIssue({ code: "custom", message: "Harness source must not reference an individual provider", path: ["providerId"] });
  }),
  voiceSpeak: z.object({
    source: z.enum(["harness", "provider"]),
    providerId: providerIdSchema.nullable(),
    timeoutMs: z.number().int().min(1_000).max(300_000),
  }).strict().superRefine((route, context) => {
    if (route.source === "provider" && !route.providerId) context.addIssue({ code: "custom", message: "Individual TTS source requires a provider", path: ["providerId"] });
    if (route.source === "harness" && route.providerId !== null) context.addIssue({ code: "custom", message: "Harness source must not reference an individual provider", path: ["providerId"] });
  }),
  codingAssist: z.object({
    providerId: z.literal("codex-sdk"),
    timeoutMs: z.number().int().min(1_000).max(300_000),
    readOnly: z.literal(true),
    networkEnabled: z.literal(false),
    webSearchEnabled: z.literal(false),
  }).strict(),
}).strict();

export const voiceSettingsSchema = z.object({
  listeningEnabled: z.boolean(),
  inputDeviceId: z.string().trim().min(1).max(300),
  outputDeviceId: z.string().trim().min(1).max(300),
  vadSensitivity: z.enum(["low", "medium", "high"]),
  silenceTimeoutMs: z.number().int().min(800).max(3000),
  allowedLanguages: z.array(z.enum(ASR_LANGUAGE_CODES)).min(1).max(ASR_LANGUAGE_CODES.length)
    .refine((languages) => new Set(languages).size === languages.length, "ASR languages must be unique"),
  autoSpeak: z.boolean(),
}).strict();

export const securitySettingsSchema = z.object({
  localOnlyWhenSelected: z.boolean(),
  diagnosticsRedaction: z.literal(true),
}).strict();

export const regionalPreferencesSchema = z.object({
  language: z.enum(DISPLAY_LANGUAGE_PREFERENCES),
  timeZone: z.string().max(100).refine(isSupportedTimeZone, "Unsupported time zone"),
  lengthUnit: z.enum(LENGTH_UNIT_SYSTEMS),
  weightUnit: z.enum(WEIGHT_UNITS),
  currency: z.enum(CURRENCY_CODES),
}).strict();

export const situationSettingsSchema = z.object({
  enabled: z.boolean(),
  sampleIntervalMs: z.number().int().min(500).max(60_000),
  calendarEnabled: z.boolean(),
  retentionDays: z.number().int().min(1).max(30),
  maxLedgerEntries: z.number().int().min(100).max(10_000),
  heartbeatIntervalMs: z.number().int().min(60_000).max(3_600_000),
  sensitiveApplicationCategories: z.literal(true),
}).strict();

const settingsDocumentBaseSchema = z.object({
  namespace: z.enum(["providers.model", "providers.agent", "routing.tasks", "voice.runtime", "security.runtime", "ui.preferences", "situation.runtime"]),
  key: z.enum(["default", "codex-sdk"]),
  schemaVersion: z.literal(12),
  valueJson: z.record(z.string(), z.unknown()),
}).strict();

export function validateSettingsDocuments(documents: unknown[]): void {
  const parsed = z.array(settingsDocumentBaseSchema).length(7).parse(documents);
  const expectedDocuments = new Set([
    "providers.model:default",
    "providers.agent:codex-sdk",
    "routing.tasks:default",
    "voice.runtime:default",
    "security.runtime:default",
    "ui.preferences:default",
    "situation.runtime:default",
  ]);
  const namespaces = new Set(parsed.map((document) => `${document.namespace}:${document.key}`));
  if (namespaces.size !== expectedDocuments.size || [...namespaces].some((value) => !expectedDocuments.has(value))) {
    throw new Error("Each supported settings document must appear exactly once");
  }
  parsed.forEach((document) => {
    switch (document.namespace) {
      case "providers.model":
        modelProvidersSettingsSchema.parse(document.valueJson);
        break;
      case "providers.agent":
        codexAgentSettingsSchema.parse(document.valueJson);
        break;
      case "routing.tasks":
        routingSettingsSchema.parse(document.valueJson);
        break;
      case "voice.runtime":
        voiceSettingsSchema.parse(document.valueJson);
        break;
      case "security.runtime":
        securitySettingsSchema.parse(document.valueJson);
        break;
      case "ui.preferences":
        regionalPreferencesSchema.parse(document.valueJson);
        break;
      case "situation.runtime":
        situationSettingsSchema.parse(document.valueJson);
        break;
    }
  });
  const providersDocument = parsed.find((document) => document.namespace === "providers.model");
  const routingDocument = parsed.find((document) => document.namespace === "routing.tasks");
  const securityDocument = parsed.find((document) => document.namespace === "security.runtime");
  const providerSettings = modelProvidersSettingsSchema.parse(providersDocument?.valueJson);
  const providers = providerSettings.providers;
  const routing = routingSettingsSchema.parse(routingDocument?.valueJson);
  const security = securitySettingsSchema.parse(securityDocument?.valueJson);
  const usesHarness = routing.conversationRespond.source === "harness"
    || routing.voiceTranscribe.source === "harness"
    || routing.voiceSpeak.source === "harness";
  if (usesHarness && !providerSettings.harness.address.trim()) {
    throw new Error("Provider Harness address is required while a Harness route is selected");
  }
  const enabled = new Map(providers.filter((provider) => provider.enabled).map((provider) => [provider.id, provider]));
  const primaryId = routing.conversationRespond.primaryProviderId;
  const primary = primaryId ? enabled.get(primaryId) : undefined;
  if (routing.conversationRespond.source === "provider" && !primary) throw new Error("The primary conversation provider must be enabled");
  if (primary && !["openai-compatible", "larm", "dynamic-lan"].includes(primary.kind)) throw new Error("The selected conversation provider does not support LLM");
  if (primary?.kind === "dynamic-lan" && routing.conversationRespond.timeoutMs > DYNAMIC_LAN_MAX_REQUEST_TIMEOUT_MS) {
    throw new Error(`dynamic LAN conversation timeout must not exceed ${DYNAMIC_LAN_MAX_REQUEST_TIMEOUT_MS} ms`);
  }
  if (routing.conversationRespond.source === "harness" && routing.conversationRespond.fallbackProviderIds.length > 0) throw new Error("Harness routes do not use individual provider fallbacks");
  const routeIds = new Set(primaryId ? [primaryId] : []);
  for (const fallbackId of routing.conversationRespond.fallbackProviderIds) {
    const fallback = enabled.get(fallbackId);
    if (!fallback) throw new Error(`Fallback provider is not enabled: ${fallbackId}`);
    if (routeIds.has(fallbackId)) throw new Error(`Duplicate provider in route: ${fallbackId}`);
    routeIds.add(fallbackId);
    if (security.localOnlyWhenSelected && primary?.location === "local" && fallback.location === "cloud") {
      throw new Error(`Cloud fallback is blocked while the local-only policy is active: ${fallbackId}`);
    }
  }
  const asrProvider = routing.voiceTranscribe.providerId ? enabled.get(routing.voiceTranscribe.providerId) : undefined;
  if (routing.voiceTranscribe.source === "provider" && asrProvider?.kind !== "cloud-asr") throw new Error("The selected provider does not support ASR");
  const ttsProvider = routing.voiceSpeak.providerId ? enabled.get(routing.voiceSpeak.providerId) : undefined;
  if (routing.voiceSpeak.source === "provider" && !ttsProvider) throw new Error("The selected TTS provider must be enabled");
  if (ttsProvider && !["cloud-tts", "system-tts"].includes(ttsProvider.kind)) throw new Error("The selected provider does not support TTS");
}
