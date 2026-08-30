import { z } from "zod";

export const providerIdSchema = z.string().min(1).max(80).regex(
  /^[A-Za-z0-9_-]+$/,
  "Provider ids may contain only ASCII letters, numbers, hyphens, and underscores",
);

const providerCommonSchema = z.object({
  id: providerIdSchema,
  enabled: z.boolean(),
  label: z.string().min(1).max(120).refine((value) => value.trim() === value && !/[\u0000-\u001f\u007f]/.test(value), "Provider labels must not have surrounding whitespace or control characters"),
});

function isLocalProviderHost(hostname: string): boolean {
  if (hostname.startsWith("[") && hostname.endsWith("]")) {
    const address = hostname.slice(1, -1);
    return address === "::1" || /^(?:fc|fd)[0-9a-f]{2}:/i.test(address);
  }
  if (hostname === "localhost" || hostname.endsWith(".local") || !hostname.includes(".")) return true;
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

function isDynamicLanHost(value: string): boolean {
  if (!value || value.length > 253 || /[\s/@?#]/.test(value) || value.includes(":")) return false;
  const octets = value.split(".").map(Number);
  if (octets.length === 4 && octets.every((octet) => Number.isInteger(octet) && octet >= 0 && octet <= 255)) {
    return octets[0] === 10
      || octets[0] === 127
      || (octets[0] === 169 && octets[1] === 254)
      || (octets[0] === 172 && octets[1] >= 16 && octets[1] <= 31)
      || (octets[0] === 192 && octets[1] === 168);
  }
  const labels = value.split(".");
  return (labels.length === 1 || value.toLowerCase().endsWith(".local"))
    && labels.every((label) => label.length >= 1
      && label.length <= 63
      && /^[A-Za-z0-9](?:[A-Za-z0-9-]*[A-Za-z0-9])?$/.test(label));
}

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

function isSafeEndpoint(value: string): boolean {
  try {
    const endpoint = new URL(value);
    return !endpoint.username && !endpoint.password && !endpoint.search && !endpoint.hash;
  } catch {
    return false;
  }
}

const harnessAddressSchema = z.union([z.literal(""), z.string().url().max(2_048)]).superRefine((value, context) => {
  if (!value) return;
  if (!isSafeEndpoint(value)) {
    context.addIssue({ code: "custom", message: "Harness address must not contain credentials, a query, or a fragment" });
    return;
  }
  const address = new URL(value);
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
        const endpoint = new URL(provider.endpoint);
        if (!isSafeEndpoint(provider.endpoint)) context.addIssue({ code: "custom", message: "Provider endpoints must not contain credentials, query, or fragment", path: [index, "endpoint"] });
        if (provider.location === "local" && (endpoint.protocol !== "http:" || !isLocalProviderHost(endpoint.hostname))) context.addIssue({ code: "custom", message: "Local providers must use an http:// loopback or private-network endpoint", path: [index, "endpoint"] });
        if (provider.location === "cloud" && endpoint.protocol !== "https:") context.addIssue({ code: "custom", message: "Cloud providers must use HTTPS", path: [index, "endpoint"] });
      }
      if ((provider.kind === "cloud-asr" || provider.kind === "cloud-tts") && provider.endpoint) {
        const endpoint = new URL(provider.endpoint);
        if (endpoint.protocol !== "https:" || !isSafeEndpoint(provider.endpoint)) context.addIssue({ code: "custom", message: "Cloud providers must use a credential-free HTTPS endpoint without query or fragment", path: [index, "endpoint"] });
      }
      if (provider.kind === "larm") {
        const baseUrl = new URL(provider.baseUrl);
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
