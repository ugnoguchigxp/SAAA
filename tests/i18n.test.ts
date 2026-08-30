import { afterEach, describe, expect, test } from "bun:test";
import i18n, {
  APP_LANGUAGE_STORAGE_KEY,
  detectInitialLanguage,
  normalizeAppLanguage,
  resolveDisplayLanguagePreference,
} from "../src/i18n";
import { en } from "../src/i18n/locales/en";
import { ja } from "../src/i18n/locales/ja";

function translationKeys(value: object, prefix = ""): string[] {
  return Object.entries(value).flatMap(([key, child]) => {
    const path = prefix ? `${prefix}.${key}` : key;
    return typeof child === "object" && child !== null
      ? translationKeys(child as object, path)
      : [path];
  });
}

afterEach(() => void i18n.changeLanguage("en"));

describe("application i18n", () => {
  test("keeps English and Japanese resources in sync", () => {
    expect(translationKeys(ja).sort()).toEqual(translationKeys(en).sort());
  });

  test("prefers a persisted supported language", () => {
    const storage = { getItem: (key: string) => key === APP_LANGUAGE_STORAGE_KEY ? "ja" : null };
    expect(detectInitialLanguage(storage, "en-US")).toBe("ja");
  });

  test("falls back to the browser language and normalizes unsupported locales", () => {
    const storage = { getItem: () => null };
    expect(detectInitialLanguage(storage, "ja-JP")).toBe("ja");
    expect(detectInitialLanguage(storage, "fr-FR")).toBe("en");
    expect(normalizeAppLanguage("JA-jp")).toBe("ja");
    expect(resolveDisplayLanguagePreference("system", "ja-JP")).toBe("ja");
    expect(resolveDisplayLanguagePreference("en", "ja-JP")).toBe("en");
  });

  test("switches the same UI key between English and Japanese", async () => {
    await i18n.changeLanguage("en");
    expect(i18n.t("app.chat")).toBe("Chat");
    await i18n.changeLanguage("ja");
    expect(i18n.t("app.chat")).toBe("会話");
  });
});
