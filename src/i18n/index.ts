import i18n from "i18next";
import { initReactI18next } from "react-i18next";
import type { DisplayLanguagePreference } from "../lib/settingsTypes";
import { en } from "./locales/en";
import { ja } from "./locales/ja";

export const APP_LANGUAGE_STORAGE_KEY = "saaa.display-language";
const APP_LANGUAGES = ["en", "ja"] as const;
export type AppLanguage = (typeof APP_LANGUAGES)[number];

export function normalizeAppLanguage(language: string | null | undefined): AppLanguage {
  return language?.toLowerCase().startsWith("ja") ? "ja" : "en";
}

export function detectInitialLanguage(
  storage: Pick<Storage, "getItem"> | null = typeof window === "undefined" ? null : window.localStorage,
  browserLanguage: string | null | undefined = typeof navigator === "undefined" ? null : navigator.language,
): AppLanguage {
  try {
    const stored = storage?.getItem(APP_LANGUAGE_STORAGE_KEY);
    if (stored === "en" || stored === "ja") return stored;
  } catch {
    // Language detection should not prevent startup when storage is unavailable.
  }
  return normalizeAppLanguage(browserLanguage);
}

function applyDocumentLanguage(language: string) {
  if (typeof document !== "undefined") document.documentElement.lang = normalizeAppLanguage(language);
}

function persistLanguage(language: string) {
  const normalized = normalizeAppLanguage(language);
  applyDocumentLanguage(normalized);
  try {
    if (typeof window !== "undefined") window.localStorage.setItem(APP_LANGUAGE_STORAGE_KEY, normalized);
  } catch {
    // The in-memory language still changes when persistent storage is unavailable.
  }
}

void i18n.use(initReactI18next).init({
  resources: {
    en: { translation: en },
    ja: { translation: ja },
  },
  lng: detectInitialLanguage(),
  fallbackLng: "en",
  supportedLngs: APP_LANGUAGES,
  load: "languageOnly",
  initAsync: false,
  interpolation: { escapeValue: false },
  react: { useSuspense: false },
});

i18n.on("languageChanged", persistLanguage);
applyDocumentLanguage(i18n.resolvedLanguage ?? i18n.language);

function setAppLanguage(language: AppLanguage): Promise<unknown> {
  return i18n.changeLanguage(language);
}

export function resolveDisplayLanguagePreference(
  preference: DisplayLanguagePreference,
  browserLanguage: string | null | undefined = typeof navigator === "undefined" ? null : navigator.language,
): AppLanguage {
  return preference === "system" ? normalizeAppLanguage(browserLanguage) : preference;
}

export function setDisplayLanguagePreference(preference: DisplayLanguagePreference): Promise<unknown> {
  return setAppLanguage(resolveDisplayLanguagePreference(preference));
}

export default i18n;
