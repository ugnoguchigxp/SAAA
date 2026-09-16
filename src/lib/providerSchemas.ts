import { z } from "zod";
import { isDynamicLanHost, isLocalProviderHost } from "./localProviderAddress";

export const providerIdSchema = z
  .string()
  .min(1)
  .max(80)
  .regex(
    /^[A-Za-z0-9_-]+$/,
    "Provider ids may contain only ASCII letters, numbers, hyphens, and underscores",
  );

const llmRequestOptionsSchema = z
  .object({
    tokenLimit: z.enum(["auto", "legacy", "completion"]).default("auto"),
    reasoning: z.enum(["auto", "supported", "unsupported"]).default("auto"),
    tools: z.boolean().default(true),
    streaming: z.boolean().default(true),
  })
  .strict();

const providerCommonSchema = z.object({
  id: providerIdSchema,
  enabled: z.boolean(),
  label: z
    .string()
    .min(1)
    .max(120)
    .refine(
      (value) => value.trim() === value && !/[\u0000-\u001f\u007f]/.test(value),
      "Provider labels must not have surrounding whitespace or control characters",
    ),
});

const openAiCompatibleProviderSchema = providerCommonSchema
  .extend({
    kind: z.literal("openai-compatible"),
    requestOptions: llmRequestOptionsSchema.optional(),
    location: z.enum(["local", "cloud"]),
    endpoint: z.union([
      z.literal(""),
      z
        .string()
        .url()
        .max(2_048)
        .refine(
          (value) => value.startsWith("http://") || value.startsWith("https://"),
          "Endpoint must use HTTP or HTTPS",
        ),
    ]),
    model: z
      .string()
      .max(160)
      .refine(
        (value) => value.trim() === value && !/[\u0000-\u001f\u007f]/.test(value),
        "Model names must not have surrounding whitespace or control characters",
      ),
    authentication: z.enum(["none", "api-key"]),
  })
  .strict();

const agentSessionProviderSchema = providerCommonSchema
  .extend({
    kind: z.literal("agent-session"),
    location: z.enum(["local", "cloud"]),
    baseUrl: z.string().url().max(2_048),
    model: z
      .string()
      .max(256)
      .refine(
        (value) => value.trim() === value && !/[\u0000-\u001f\u007f]/.test(value),
        "Model names must not have surrounding whitespace or control characters",
      ),
    modelsPath: z.string().min(1).max(2_048),
    sessionsPath: z.string().min(1).max(2_048),
    authentication: z.enum(["none", "api-key"]),
  })
  .strict();

const cloudAsrProviderSchema = providerCommonSchema
  .extend({
    kind: z.literal("cloud-asr"),
    location: z.enum(["local", "cloud"]),
    endpoint: z.string().url().max(2_048),
    model: z
      .string()
      .min(1)
      .max(160)
      .refine(
        (value) => value.trim() === value && !/[\u0000-\u001f\u007f]/.test(value),
        "Model names must not have surrounding whitespace or control characters",
      ),
    language: z.literal("auto"),
    authentication: z.enum(["none", "api-key"]),
  })
  .strict();

const cloudTtsProviderSchema = providerCommonSchema
  .extend({
    kind: z.literal("cloud-tts"),
    responseFormat: z.enum(["wav", "pcm"]).default("wav"),
    location: z.enum(["local", "cloud"]),
    endpoint: z.string().url().max(2_048),
    model: z
      .string()
      .min(1)
      .max(160)
      .refine(
        (value) => value.trim() === value && !/[\u0000-\u001f\u007f]/.test(value),
        "Model names must not have surrounding whitespace or control characters",
      ),
    voice: z
      .string()
      .min(1)
      .max(160)
      .refine(
        (value) => value.trim() === value && !/[\u0000-\u001f\u007f]/.test(value),
        "Voice names must not have surrounding whitespace or control characters",
      ),
    authentication: z.enum(["none", "api-key"]),
  })
  .strict();

const systemTtsProviderSchema = providerCommonSchema
  .extend({
    kind: z.literal("system-tts"),
    location: z.literal("local"),
    voice: z
      .string()
      .min(1)
      .max(160)
      .refine(
        (value) => value.trim() === value && !/[\u0000-\u001f\u007f]/.test(value),
        "Voice names must not have surrounding whitespace or control characters",
      ),
  })
  .strict();

const dynamicLanProviderSchema = providerCommonSchema
  .extend({
    kind: z.literal("dynamic-lan"),
    requestOptions: llmRequestOptionsSchema.optional(),
    location: z.literal("local"),
    host: z
      .string()
      .trim()
      .refine(
        isDynamicLanHost,
        "dynamic_lan host must be a private IP, .local name, or single-label hostname without a scheme, port, or path",
      ),
  })
  .strict();

const providerSchema = z.discriminatedUnion("kind", [
  openAiCompatibleProviderSchema,
  agentSessionProviderSchema,
  cloudAsrProviderSchema,
  cloudTtsProviderSchema,
  systemTtsProviderSchema,
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

function isSafeRelativeProviderPath(value: string, allowQuery: boolean): boolean {
  if (!value.startsWith("/") || value.startsWith("//") || /[\u0000-\u001f\u007f]/.test(value))
    return false;
  try {
    const parsed = new URL(value, "http://provider.invalid");
    return (
      parsed.origin === "http://provider.invalid" &&
      !parsed.username &&
      !parsed.password &&
      !parsed.hash &&
      (allowQuery || !parsed.search)
    );
  } catch {
    return false;
  }
}

const harnessAddressSchema = z
  .union([z.literal(""), z.string().url().max(2_048)])
  .superRefine((value, context) => {
    if (!value) return;
    const address = parsedUrl(value);
    if (!address) return;
    if (!isSafeEndpoint(address)) {
      context.addIssue({
        code: "custom",
        message: "Harness address must not contain credentials, a query, or a fragment",
      });
      return;
    }
    if (address.protocol !== "http:" && address.protocol !== "https:") {
      context.addIssue({ code: "custom", message: "Harness address must use HTTP or HTTPS" });
    }
    if (address.protocol === "http:" && !isLocalProviderHost(address.hostname)) {
      context.addIssue({ code: "custom", message: "Public harness addresses must use HTTPS" });
    }
  });

export const modelProvidersSettingsSchema = z
  .object({
    harness: z
      .object({
        address: harnessAddressSchema,
        larmProfile: z
          .string()
          .min(1)
          .max(160)
          .regex(/^[A-Za-z0-9_-]+$/)
          .optional(),
        ttsVoice: z
          .string()
          .min(1)
          .max(160)
          .refine((v) => v.trim() === v && !/[\u0000-\u001f\u007f]/.test(v))
          .optional(),
      })
      .strict(),
    providers: z
      .array(providerSchema)
      .max(20)
      .superRefine((providers, context) => {
        const ids = new Set<string>();
        let enabledDynamicLanProviders = 0;
        providers.forEach((provider, index) => {
          if (ids.has(provider.id))
            context.addIssue({
              code: "custom",
              message: `Duplicate provider id: ${provider.id}`,
              path: [index, "id"],
            });
          ids.add(provider.id);
          if (provider.kind === "dynamic-lan" && provider.enabled) enabledDynamicLanProviders += 1;
          if (
            provider.kind === "openai-compatible" &&
            provider.enabled &&
            (!provider.endpoint || !provider.model)
          )
            context.addIssue({
              code: "custom",
              message: "Enabled providers require an endpoint and model",
              path: [index],
            });
          if (provider.kind === "openai-compatible" && provider.endpoint) {
            const endpoint = parsedUrl(provider.endpoint);
            if (!endpoint) return;
            if (!isSafeEndpoint(endpoint))
              context.addIssue({
                code: "custom",
                message: "Provider endpoints must not contain credentials, query, or fragment",
                path: [index, "endpoint"],
              });
            if (
              provider.location === "local" &&
              (endpoint.protocol !== "http:" || !isLocalProviderHost(endpoint.hostname))
            )
              context.addIssue({
                code: "custom",
                message: "Local providers must use an http:// loopback or private-network endpoint",
                path: [index, "endpoint"],
              });
            if (provider.location === "cloud" && endpoint.protocol !== "https:")
              context.addIssue({
                code: "custom",
                message: "Cloud providers must use HTTPS",
                path: [index, "endpoint"],
              });
          }
          if (provider.kind === "agent-session") {
            if (provider.enabled && !provider.model)
              context.addIssue({
                code: "custom",
                message: "Enabled Agent Session providers require a model",
                path: [index, "model"],
              });
            const baseUrl = parsedUrl(provider.baseUrl);
            if (!baseUrl) return;
            if (!isSafeEndpoint(baseUrl) || baseUrl.pathname !== "/")
              context.addIssue({
                code: "custom",
                message:
                  "Agent Session base URL must be an origin without credentials, path, query, or fragment",
                path: [index, "baseUrl"],
              });
            if (
              provider.location === "local" &&
              (baseUrl.protocol !== "http:" || !isLocalProviderHost(baseUrl.hostname))
            )
              context.addIssue({
                code: "custom",
                message:
                  "Local Agent Session providers must use an http:// loopback or private-network base URL",
                path: [index, "baseUrl"],
              });
            if (provider.location === "cloud" && baseUrl.protocol !== "https:")
              context.addIssue({
                code: "custom",
                message: "Cloud Agent Session providers must use HTTPS",
                path: [index, "baseUrl"],
              });
            if (
              !isSafeRelativeProviderPath(provider.modelsPath, true) ||
              !new URL(provider.modelsPath, baseUrl).searchParams.get("runtime")
            )
              context.addIssue({
                code: "custom",
                message:
                  "Agent Session model paths must be relative and include a runtime query parameter",
                path: [index, "modelsPath"],
              });
            if (!isSafeRelativeProviderPath(provider.sessionsPath, false))
              context.addIssue({
                code: "custom",
                message:
                  "Agent Session session paths must be relative and must not include a query or fragment",
                path: [index, "sessionsPath"],
              });
          }
          if (
            (provider.kind === "cloud-asr" || provider.kind === "cloud-tts") &&
            provider.endpoint
          ) {
            const endpoint = parsedUrl(provider.endpoint);
            if (!endpoint) return;
            if (
              !isSafeEndpoint(endpoint) ||
              (provider.location === "cloud" && endpoint.protocol !== "https:") ||
              (provider.location === "local" && !isLocalProviderHost(endpoint.hostname))
            )
              context.addIssue({
                code: "custom",
                message: "HTTP audio providers require a safe local URL or a cloud HTTPS URL",
                path: [index, "endpoint"],
              });
          }
        });
        if (enabledDynamicLanProviders > 1)
          context.addIssue({
            code: "custom",
            message: "Only one dynamic LAN provider may be enabled",
            path: [],
          });
      }),
    reasoningEffort: z.enum(["provider-default", "low", "medium", "xhigh"]),
  })
  .strict();
