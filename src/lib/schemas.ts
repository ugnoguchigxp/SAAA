import { z } from "zod";

export const runtimeFailureCodeSchema = z.enum([
  "runtime_error",
  "configuration-error",
  "child-start-failed",
  "request-timeout",
  "progress-timeout",
  "terminal-timeout",
  "hard-timeout",
  "child-exited",
  "protocol-error",
  "policy-violation",
  "provider-error",
  "response-too-large",
  "internal-error",
]);

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

const providerIdSchema = z.string().min(1).max(80).regex(
  /^[A-Za-z0-9_-]+$/,
  "Provider ids may contain only ASCII letters, numbers, hyphens, and underscores",
);

const providerCommonSchema = z.object({
  id: providerIdSchema,
  enabled: z.boolean(),
  label: z.string().trim().min(1).max(120),
});

function isLocalProviderHost(hostname: string): boolean {
  if (["localhost", "[::1]"].includes(hostname)) return true;
  const octets = hostname.split(".").map(Number);
  if (octets.length !== 4 || octets.some((octet) => !Number.isInteger(octet) || octet < 0 || octet > 255)) return false;
  return octets[0] === 10
    || octets[0] === 127
    || (octets[0] === 172 && octets[1] >= 16 && octets[1] <= 31)
    || (octets[0] === 192 && octets[1] === 168);
}

const openAiCompatibleProviderSchema = providerCommonSchema.extend({
  kind: z.literal("openai-compatible"),
  location: z.enum(["local", "cloud"]),
  endpoint: z.union([z.literal(""), z.string().url().max(2_048).refine((value) => value.startsWith("http://") || value.startsWith("https://"), "Endpoint must use HTTP or HTTPS")]),
  model: z.string().trim().max(160),
  credentialStatus: z.enum(["not-configured", "configured"]),
}).strict();

const larmProviderSchema = providerCommonSchema.extend({
  kind: z.literal("larm"),
  location: z.literal("local"),
  baseUrl: z.string().url().max(2_048),
  tokenEnv: z.literal("LARM_API_TOKEN"),
  allocationTtlSeconds: z.number().int().min(60).max(3_600),
  allocationStartupTimeoutSeconds: z.number().int().min(1).max(300),
  allowFallbackByDefault: z.literal(false),
  deploymentPolicy: z.literal("existing-only"),
}).strict();

const providerSchema = z.discriminatedUnion("kind", [
  openAiCompatibleProviderSchema,
  larmProviderSchema,
]);

export const modelProvidersSettingsSchema = z.object({
  providers: z.array(providerSchema).min(1).max(20).superRefine((providers, context) => {
    const ids = new Set<string>();
    const credentialSuffixes = new Set<string>();
    let enabledLarmProviders = 0;
    providers.forEach((provider, index) => {
      if (ids.has(provider.id)) {
        context.addIssue({ code: "custom", message: `Duplicate provider id: ${provider.id}`, path: [index, "id"] });
      }
      ids.add(provider.id);
      const credentialSuffix = provider.id.toUpperCase().replace(/[^A-Z0-9]/g, "_");
      if (credentialSuffixes.has(credentialSuffix)) {
        context.addIssue({ code: "custom", message: "Provider id maps to an existing credential environment variable", path: [index, "id"] });
      }
      credentialSuffixes.add(credentialSuffix);
      if (provider.kind === "larm" && provider.enabled) enabledLarmProviders += 1;
      if (provider.kind === "openai-compatible" && provider.enabled && (!provider.endpoint || !provider.model)) {
        context.addIssue({ code: "custom", message: "Enabled providers require an endpoint and model", path: [index] });
      }
      if (provider.kind === "openai-compatible" && provider.endpoint) {
        const endpoint = new URL(provider.endpoint);
        if (endpoint.username || endpoint.password) {
          context.addIssue({ code: "custom", message: "Credentials must not be embedded in endpoints", path: [index, "endpoint"] });
        }
        if (provider.location === "local" && (endpoint.protocol !== "http:" || !isLocalProviderHost(endpoint.hostname))) {
          context.addIssue({ code: "custom", message: "Local providers must use an http:// loopback or private-network endpoint", path: [index, "endpoint"] });
        }
        if (provider.location === "cloud" && endpoint.protocol !== "https:") {
          context.addIssue({ code: "custom", message: "Cloud providers must use HTTPS", path: [index, "endpoint"] });
        }
      }
      if (provider.kind === "larm") {
        const baseUrl = new URL(provider.baseUrl);
        if (
          baseUrl.protocol !== "http:" ||
          !["127.0.0.1", "[::1]"].includes(baseUrl.hostname) ||
          !baseUrl.port ||
          baseUrl.username ||
          baseUrl.password ||
          baseUrl.search ||
          baseUrl.hash ||
          baseUrl.pathname !== "/"
        ) {
          context.addIssue({ code: "custom", message: "LARM must use an explicit HTTP numeric-loopback base URL without credentials, path, query, or fragment", path: [index, "baseUrl"] });
        }
      }
    });
    if (enabledLarmProviders > 1) {
      context.addIssue({ code: "custom", message: "Only one LARM provider may be enabled", path: [] });
    }
  }),
}).strict();

export const codexAgentSettingsSchema = z.object({
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
    primaryProviderId: providerIdSchema,
    fallbackProviderIds: z.array(providerIdSchema).max(20),
    timeoutMs: z.number().int().min(1_000).max(300_000),
  }).strict(),
  codingAssist: z.object({
    providerId: z.literal("codex-sdk"),
    timeoutMs: z.number().int().min(1_000).max(300_000),
    readOnly: z.literal(true),
    networkEnabled: z.literal(false),
    webSearchEnabled: z.literal(false),
  }).strict(),
}).strict();

export const voiceSettingsSchema = z.object({
  inputDeviceId: z.string().trim().min(1).max(300),
  outputDeviceId: z.string().trim().min(1).max(300),
  captureMode: z.literal("push-to-talk"),
  sttProviderId: z.literal("gnosis-asr"),
  sttModel: z.literal("qwen3-asr-1.7b"),
  ttsProviderId: z.literal("system-tts"),
  ttsVoice: z.string().trim().min(1).max(160),
  autoSpeak: z.boolean(),
  cloudFallbackEnabled: z.literal(false),
}).strict();

export const securitySettingsSchema = z.object({
  credentialStorage: z.literal("environment"),
  localOnlyWhenSelected: z.boolean(),
  diagnosticsRedaction: z.literal(true),
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
  namespace: z.enum(["providers.model", "providers.agent", "routing.tasks", "voice.runtime", "security.runtime", "situation.runtime"]),
  key: z.enum(["default", "codex-sdk"]),
  schemaVersion: z.literal(9),
  valueJson: z.record(z.string(), z.unknown()),
}).strict();

export function validateSettingsDocuments(documents: unknown[]): void {
  const parsed = z.array(settingsDocumentBaseSchema).length(6).parse(documents);
  const expectedDocuments = new Set([
    "providers.model:default",
    "providers.agent:codex-sdk",
    "routing.tasks:default",
    "voice.runtime:default",
    "security.runtime:default",
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
      case "situation.runtime":
        situationSettingsSchema.parse(document.valueJson);
        break;
    }
  });
  const providersDocument = parsed.find((document) => document.namespace === "providers.model");
  const routingDocument = parsed.find((document) => document.namespace === "routing.tasks");
  const securityDocument = parsed.find((document) => document.namespace === "security.runtime");
  const providers = modelProvidersSettingsSchema.parse(providersDocument?.valueJson).providers;
  const routing = routingSettingsSchema.parse(routingDocument?.valueJson);
  const security = securitySettingsSchema.parse(securityDocument?.valueJson);
  const enabled = new Map(providers.filter((provider) => provider.enabled).map((provider) => [provider.id, provider]));
  const primary = enabled.get(routing.conversationRespond.primaryProviderId);
  if (enabled.size > 0 && !primary) throw new Error("The primary conversation provider must be enabled");
  const routeIds = new Set([routing.conversationRespond.primaryProviderId]);
  for (const fallbackId of routing.conversationRespond.fallbackProviderIds) {
    const fallback = enabled.get(fallbackId);
    if (!fallback) throw new Error(`Fallback provider is not enabled: ${fallbackId}`);
    if (routeIds.has(fallbackId)) throw new Error(`Duplicate provider in route: ${fallbackId}`);
    routeIds.add(fallbackId);
    if (security.localOnlyWhenSelected && primary?.location === "local" && fallback.location === "cloud") {
      throw new Error(`Cloud fallback is blocked while the local-only policy is active: ${fallbackId}`);
    }
  }
}
