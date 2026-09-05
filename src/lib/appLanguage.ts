import { setDisplayLanguagePreference } from "../i18n";
import { findSettingsDocument, type AppSnapshot } from "./contracts";
import { regionalPreferencesSchema } from "./schemas";

export function applySnapshotLanguage(snapshot: AppSnapshot) {
  const document = findSettingsDocument(snapshot.settings, "ui.preferences", "default");
  const parsed = regionalPreferencesSchema.safeParse(document?.valueJson);
  if (parsed.success) {
    void setDisplayLanguagePreference(parsed.data.language);
  }
}
