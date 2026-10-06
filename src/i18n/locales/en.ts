import { enCore } from "./enCore";
import { enDiagnosis } from "./enDiagnosis";
import { enSettings } from "./enSettings";
import { enVoice } from "./enVoice";
export const en = {
  ...enCore,
  ...enSettings,
  ...enVoice,
  navigation: { ...enCore.navigation, diagnosis: "Diagnosis", workers: "Worker Agents" },
  chat: { ...enCore.chat, diagnosis: enDiagnosis },
} as const;
