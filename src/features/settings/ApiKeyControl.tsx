import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import type {
  AgentSessionProviderSettings,
  CloudAsrProviderSettings,
  CloudTtsProviderSettings,
  OpenAiCompatibleProviderSettings,
  ProviderCredentialState,
} from "../../lib/contracts";
import {
  deleteProviderApiKey,
  getProviderCredentialState,
  setProviderApiKey,
} from "../../lib/runtime";
import { localizeUiMessage } from "../../i18n/presentation";
import { credentialStorageSupport } from "./providerTestPresentation";

type CredentialProvider =
  | OpenAiCompatibleProviderSettings
  | AgentSessionProviderSettings
  | CloudAsrProviderSettings
  | CloudTtsProviderSettings;

export function ApiKeyControl({
  provider,
  persisted,
  onCredentialChange,
}: {
  provider: CredentialProvider;
  persisted: boolean;
  onCredentialChange: () => void;
}) {
  const { t } = useTranslation();
  const [credential, setCredential] = useState<ProviderCredentialState["state"]>("missing");
  const [apiKey, setApiKey] = useState("");
  const [saving, setSaving] = useState(false);
  const [credentialError, setCredentialError] = useState<string | null>(null);
  const storageSupport = credentialStorageSupport(globalThis.navigator?.userAgent ?? "");

  useEffect(() => {
    if (provider.authentication !== "api-key" || !persisted || storageSupport === "unsupported")
      return;
    let active = true;
    setCredentialError(null);
    void getProviderCredentialState(provider.id)
      .then((result) => {
        if (active) setCredential(result.state);
      })
      .catch((cause) => {
        if (!active) return;
        setCredential("unavailable");
        setCredentialError(cause instanceof Error ? cause.message : String(cause));
      });
    return () => {
      active = false;
    };
  }, [persisted, provider.authentication, provider.id, storageSupport]);

  if (provider.authentication !== "api-key") return <span>{t("settings.providers.authNone")}</span>;
  if (!persisted) return <span>{t("settings.providers.saveBeforeKey")}</span>;
  if (storageSupport === "unsupported")
    return (
      <span className="provider-test-result error" role="status">
        {t("settings.providers.keyStorageUnsupported")}
      </span>
    );

  async function save() {
    onCredentialChange();
    setSaving(true);
    setCredentialError(null);
    try {
      const result = await setProviderApiKey(provider.id, apiKey);
      onCredentialChange();
      setCredential(result.state);
      setApiKey("");
    } catch (cause) {
      setCredentialError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      setSaving(false);
    }
  }

  async function remove() {
    onCredentialChange();
    setSaving(true);
    setCredentialError(null);
    try {
      const result = await deleteProviderApiKey(provider.id);
      onCredentialChange();
      setCredential(result.state);
      setApiKey("");
    } catch (cause) {
      setCredentialError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      setSaving(false);
    }
  }

  return (
    <div>
      <span>
        {t("settings.providers.apiKeyState", {
          state: t(`common.${credential}`, { defaultValue: credential }),
        })}
      </span>
      <p className="muted">{t("settings.providers.keyStorageMacOnly")}</p>
      {credentialError && (
        <p className="provider-test-result error" aria-live="polite">
          {localizeUiMessage(t, credentialError, "settings")}
        </p>
      )}
      <div>
        <input
          type="password"
          value={apiKey}
          autoComplete="off"
          placeholder={
            credential === "configured"
              ? t("settings.providers.replaceKey")
              : t("settings.providers.enterKey")
          }
          onChange={(event) => setApiKey(event.target.value)}
        />
        <button
          className="text-button"
          type="button"
          disabled={!apiKey || saving}
          onClick={() => void save()}
        >
          {t("settings.providers.saveKey")}
        </button>
        {credential === "configured" && (
          <button
            className="text-button danger"
            type="button"
            disabled={saving}
            onClick={() => void remove()}
          >
            {t("settings.providers.deleteKey")}
          </button>
        )}
      </div>
    </div>
  );
}
