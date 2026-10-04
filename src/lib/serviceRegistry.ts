import { invoke } from "@tauri-apps/api/core";

export type PurposeId =
  "conversation.respond" | "voice.transcribe" | "voice.speak";
export type CapabilityId = "text-generation" | "transcription" | "speech";
export type AdapterKind =
  | "larm"
  | "chat-completions"
  | "agent-session"
  | "http-asr"
  | "http-tts"
  | "system-tts";

export type ServiceConnection = {
  connectionId: string;
  label: string;
  adapterKind: AdapterKind;
  endpoint: string;
  location: "local" | "cloud";
  authentication: "none" | "api-key";
  credentialRef?: { service: string; account: string };
  enabled: boolean;
};

export type ServiceResource = {
  resourceId: string;
  connectionId: string;
  capability: CapabilityId;
  model: string;
  detail?: string;
  enabled: boolean;
};

export type PurposeBinding = {
  purpose: PurposeId;
  enabled: boolean;
  primaryResourceId: string | null;
  fallbackResourceIds: string[];
  timeoutMs: number;
  attemptTimeoutMs?: number;
  storedPrimaryResourceId?: string;
  review: "ready" | "needs-review";
};

export type RegistrySnapshot = {
  connections: ServiceConnection[];
  resources: ServiceResource[];
  bindings: PurposeBinding[];
};

export type RegistryView = {
  snapshot: RegistrySnapshot;
  revision: number;
  persisted: boolean;
};

export const SERVICE_CREDENTIAL_SERVICE = "com.saaa.service-connection";

export const PURPOSES: Array<{
  id: PurposeId;
  label: string;
  capability: CapabilityId;
}> = [
  {
    id: "conversation.respond",
    label: "会話と回答",
    capability: "text-generation",
  },
  { id: "voice.transcribe", label: "声を聞く", capability: "transcription" },
  { id: "voice.speak", label: "声で返す", capability: "speech" },
];

export const getServiceRegistry = () =>
  invoke<RegistryView>("get_service_registry");

export const saveServiceRegistry = (
  snapshot: RegistrySnapshot,
  expectedRevision: number,
) =>
  invoke<RegistryView>("save_service_registry", { snapshot, expectedRevision });

export const setServiceConnectionSecret = (
  connectionId: string,
  apiKey: string,
) =>
  invoke<{ connectionId: string; state: string }>(
    "set_service_connection_secret",
    {
      connectionId,
      apiKey,
    },
  );

export const getServiceConnectionSecretState = (connectionId: string) =>
  invoke<{ connectionId: string; state: "configured" | "missing" }>(
    "get_service_connection_secret_state",
    { connectionId },
  );

export type NewChatService = {
  label: string;
  endpoint: string;
  model: string;
  authentication: "none" | "api-key";
  location: "local" | "cloud";
};

function slug(label: string): string {
  const text = label
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-+|-+$/g, "")
    .slice(0, 40);
  return text || "service";
}

/** Adds a disabled draft: enable it after the API key is stored and checked. */
export function addChatService(
  snapshot: RegistrySnapshot,
  input: NewChatService,
): { snapshot: RegistrySnapshot; connectionId: string; resourceId: string } {
  const base = slug(input.label);
  let suffix = 0;
  const taken = new Set(
    snapshot.connections.map((connection) => connection.connectionId),
  );
  let connectionId = `conn:svc-${base}`;
  while (taken.has(connectionId)) connectionId = `conn:svc-${base}-${++suffix}`;
  const resourceId = connectionId.replace("conn:", "res:");
  const connection: ServiceConnection = {
    connectionId,
    label: input.label.trim(),
    adapterKind: "chat-completions",
    endpoint: input.endpoint.trim().replace(/\/+$/, ""),
    location: input.location,
    authentication: input.authentication,
    ...(input.authentication === "api-key"
      ? {
          credentialRef: {
            service: SERVICE_CREDENTIAL_SERVICE,
            account: connectionId,
          },
        }
      : {}),
    enabled: false,
  };
  const resource: ServiceResource = {
    resourceId,
    connectionId,
    capability: "text-generation",
    model: input.model.trim(),
    enabled: false,
  };
  return {
    snapshot: {
      ...snapshot,
      connections: [...snapshot.connections, connection],
      resources: [...snapshot.resources, resource],
    },
    connectionId,
    resourceId,
  };
}

export function setConnectionEnabled(
  snapshot: RegistrySnapshot,
  connectionId: string,
  enabled: boolean,
): RegistrySnapshot {
  return {
    ...snapshot,
    connections: snapshot.connections.map((item) =>
      item.connectionId === connectionId ? { ...item, enabled } : item,
    ),
    resources: snapshot.resources.map((item) =>
      item.connectionId === connectionId ? { ...item, enabled } : item,
    ),
  };
}

export function candidatesFor(snapshot: RegistrySnapshot, purpose: PurposeId) {
  const capability = PURPOSES.find((item) => item.id === purpose)?.capability;
  return snapshot.resources
    .filter((resource) => resource.capability === capability)
    .map((resource) => ({
      resource,
      connection: snapshot.connections.find(
        (item) => item.connectionId === resource.connectionId,
      ),
    }));
}

/** Applying a selection is explicit: it clears needs-review and stored legacy value. */
export function assignPrimary(
  snapshot: RegistrySnapshot,
  purpose: PurposeId,
  resourceId: string | null,
): RegistrySnapshot {
  return {
    ...snapshot,
    bindings: snapshot.bindings.map((binding) => {
      if (binding.purpose !== purpose) return binding;
      const { storedPrimaryResourceId: _stored, ...rest } = binding;
      return {
        ...rest,
        enabled: resourceId !== null,
        primaryResourceId: resourceId,
        fallbackResourceIds: resourceId
          ? binding.fallbackResourceIds.filter((id) => id !== resourceId)
          : [],
        review: "ready",
      };
    }),
  };
}

/** Resources a connection removal would leave unbound. */
export function purposesUsing(
  snapshot: RegistrySnapshot,
  connectionId: string,
): PurposeId[] {
  const resourceIds = new Set(
    snapshot.resources
      .filter((resource) => resource.connectionId === connectionId)
      .map((resource) => resource.resourceId),
  );
  return snapshot.bindings
    .filter((binding) =>
      [binding.primaryResourceId, ...binding.fallbackResourceIds].some(
        (id) => id !== null && resourceIds.has(id),
      ),
    )
    .map((binding) => binding.purpose);
}
