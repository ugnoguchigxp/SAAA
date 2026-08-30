import { setDisplayLanguagePreference } from "../i18n";
import { findSettingsDocument, isRegionalPreferencesSettings, type AppSnapshot } from "./contracts";

export function applySnapshotLanguage(snapshot: AppSnapshot) {
  const document = findSettingsDocument(snapshot.settings, "ui.preferences", "default");
  if (document && isRegionalPreferencesSettings(document.valueJson)) {
    void setDisplayLanguagePreference(document.valueJson.language);
  }
}
