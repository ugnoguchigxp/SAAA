import { enCore } from "./enCore";
import { enDiagnosis } from "./enDiagnosis";
import { enSettings } from "./enSettings";
import { enVoice } from "./enVoice";
export const en = {
  ...enCore,
  ...enSettings,
  ...enVoice,
  chat: { ...enCore.chat, diagnosis: enDiagnosis },
} as const;
