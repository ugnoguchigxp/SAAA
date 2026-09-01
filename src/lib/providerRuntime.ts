import { invoke } from "@tauri-apps/api/core";
import type { HarnessResolution, ProviderCredentialState } from "./contracts";

export { legacyDynamicLanHost } from "./localProviderAddress";

export async function resolveServiceHarness(address: string): Promise<HarnessResolution> {
  return invoke<HarnessResolution>("resolve_service_harness", { address });
}

export async function setProviderApiKey(providerId: string, apiKey: string): Promise<ProviderCredentialState> {
  return invoke<ProviderCredentialState>("set_provider_api_key", { input: { providerId, apiKey } });
}

export async function deleteProviderApiKey(providerId: string): Promise<ProviderCredentialState> {
  return invoke<ProviderCredentialState>("delete_provider_api_key", { providerId });
}

export async function getProviderCredentialState(providerId: string): Promise<ProviderCredentialState> {
  return invoke<ProviderCredentialState>("get_provider_credential_state", { providerId });
}
