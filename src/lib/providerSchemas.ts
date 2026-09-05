import { z } from "zod";
import { isDynamicLanHost, isLocalProviderHost } from "./localProviderAddress";

export const providerIdSchema = z.string().min(1).max(80).regex(
  /^[A-Za-z0-9_-]+$/,
  "Provider ids may contain only ASCII letters, numbers, hyphens, and underscores",
);

const providerCommonSchema = z.object({
  id: providerIdSchema,
  enabled: z.boolean(),
  label: z.string().min(1).max(120).refine((value) => value.trim() === value && !/[\u0000-\u001f\u007f]/.test(value), "Provider labels must not have surrounding whitespace or control characters"),
});

const openAiCompatibleProviderSchema = providerCommonSchema.extend({
  kind: z.literal("openai-compatible"),
  location: z.enum(["local", "cloud"]),
  endpoint: z.union([z.literal(""), z.string().url().max(2_048).refine((value) => value.startsWith("http://") || value.startsWith("https://"), "Endpoint must use HTTP or HTTPS")]),
  model: z.string().max(160).refine((value) => value.trim() === value && !/[\u0000-\u001f\u007f]/.test(value), "Model names must not have surrounding whitespace or control characters"),
  authentication: z.enum(["none", "api-key"]),
}).strict();

const cloudAsrProviderSchema = providerCommonSchema.extend({
  kind: z.literal("cloud-asr"),
  location: z.literal("cloud"),
  endpoint: z.string().url().max(2_048),
  model: z.string().min(1).max(160).refine((value) => value.trim() === value && !/[\u0000-\u001f\u007f]/.test(value), "Model names must not have surrounding whitespace or control characters"),
  language: z.literal("auto"),
  authentication: z.enum(["none", "api-key"]),
}).strict();

const cloudTtsProviderSchema = providerCommonSchema.extend({
  kind: z.literal("cloud-tts"),
  location: z.literal("cloud"),
  endpoint: z.string().url().max(2_048),
  model: z.string().min(1).max(160).refine((value) => value.trim() === value && !/[\u0000-\u001f\u007f]/.test(value), "Model names must not have surrounding whitespace or control characters"),
  voice: z.string().min(1).max(160).refine((value) => value.trim() === value && !/[\u0000-\u001f\u007f]/.test(value), "Voice names must not have surrounding whitespace or control characters"),
  authentication: z.enum(["none", "api-key"]),
}).strict();

const systemTtsProviderSchema = providerCommonSchema.extend({
  kind: z.literal("system-tts"),
  location: z.literal("local"),
  voice: z.string().min(1).max(160).refine((value) => value.trim() === value && !/[\u0000-\u001f\u007f]/.test(value), "Voice names must not have surrounding whitespace or control characters"),
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

const dynamicLanProviderSchema = providerCommonSchema.extend({
  kind: z.literal("dynamic-lan"),
  location: z.literal("local"),
  host: z.string().trim().refine(isDynamicLanHost, "dynamic_lan host must be a private IP, .local name, or single-label hostname without a scheme, port, or path"),
}).strict();

const providerSchema = z.discriminatedUnion("kind", [
  openAiCompatibleProviderSchema,
  cloudAsrProviderSchema,
  cloudTtsProviderSchema,
  systemTtsProviderSchema,
  larmProviderSchema,
  dynamicLanProviderSchema,
]);

function isSafeEndpoint(endpoint: URL): boolean {
  return !endpoint.username && !endpoint.password && !endpoint.search && !endpoint.hash;
}

function parsedUrl(value: string): URL | null {
  try {
    return new URL(value);
  } catch {
    return null;
  }
}

const harnessAddressSchema = z.union([z.literal(""), z.string().url().max(2_048)]).superRefine((value, context) => {
  if (!value) return;
  const address = parsedUrl(value);
  if (!address) return;
  if (!isSafeEndpoint(address)) {
    context.addIssue({ code: "custom", message: "Harness address must not contain credentials, a query, or a fragment" });
    return;
  }
  if (address.protocol !== "http:" && address.protocol !== "https:") {
    context.addIssue({ code: "custom", message: "Harness address must use HTTP or HTTPS" });
  }
  if (address.protocol === "http:" && !isLocalProviderHost(address.hostname)) {
    context.addIssue({ code: "custom", message: "Public harness addresses must use HTTPS" });
  }
});

export const modelProvidersSettingsSchema = z.object({
  harness: z.object({ address: harnessAddressSchema }).strict(),
  providers: z.array(providerSchema).min(1).max(20).superRefine((providers, context) => {
    const ids = new Set<string>();
    let enabledLarmProviders = 0;
    let enabledDynamicLanProviders = 0;
    providers.forEach((provider, index) => {
      if (ids.has(provider.id)) context.addIssue({ code: "custom", message: `Duplicate provider id: ${provider.id}`, path: [index, "id"] });
      ids.add(provider.id);
      if (provider.kind === "larm" && provider.enabled) enabledLarmProviders += 1;
      if (provider.kind === "dynamic-lan" && provider.enabled) enabledDynamicLanProviders += 1;
      if (provider.kind === "openai-compatible" && provider.enabled && (!provider.endpoint || !provider.model)) context.addIssue({ code: "custom", message: "Enabled providers require an endpoint and model", path: [index] });
      if (provider.kind === "openai-compatible" && provider.endpoint) {
        const endpoint = parsedUrl(provider.endpoint);
        if (!endpoint) return;
        if (!isSafeEndpoint(endpoint)) context.addIssue({ code: "custom", message: "Provider endpoints must not contain credentials, query, or fragment", path: [index, "endpoint"] });
        if (provider.location === "local" && (endpoint.protocol !== "http:" || !isLocalProviderHost(endpoint.hostname))) context.addIssue({ code: "custom", message: "Local providers must use an http:// loopback or private-network endpoint", path: [index, "endpoint"] });
        if (provider.location === "cloud" && endpoint.protocol !== "https:") context.addIssue({ code: "custom", message: "Cloud providers must use HTTPS", path: [index, "endpoint"] });
      }
      if ((provider.kind === "cloud-asr" || provider.kind === "cloud-tts") && provider.endpoint) {
        const endpoint = parsedUrl(provider.endpoint);
        if (!endpoint) return;
        if (endpoint.protocol !== "https:" || !isSafeEndpoint(endpoint)) context.addIssue({ code: "custom", message: "Cloud providers must use a credential-free HTTPS endpoint without query or fragment", path: [index, "endpoint"] });
      }
      if (provider.kind === "larm") {
        const baseUrl = parsedUrl(provider.baseUrl);
        if (!baseUrl) return;
        if (baseUrl.protocol !== "http:" || !["127.0.0.1", "[::1]"].includes(baseUrl.hostname) || !baseUrl.port || baseUrl.username || baseUrl.password || baseUrl.search || baseUrl.hash || baseUrl.pathname !== "/") {
          context.addIssue({ code: "custom", message: "LARM must use an explicit HTTP numeric-loopback base URL without credentials, path, query, or fragment", path: [index, "baseUrl"] });
        }
      }
    });
    if (enabledLarmProviders > 1) context.addIssue({ code: "custom", message: "Only one LARM provider may be enabled", path: [] });
    if (enabledDynamicLanProviders > 1) context.addIssue({ code: "custom", message: "Only one dynamic LAN provider may be enabled", path: [] });
  }),
  reasoningEffort: z.enum(["low", "medium", "xhigh"]),
}).strict();
