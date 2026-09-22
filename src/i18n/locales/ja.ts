import { jaCore } from "./jaCore";
import { jaDiagnosis } from "./jaDiagnosis";
import { jaSettings } from "./jaSettings";
import { jaVoice } from "./jaVoice";
export const ja = {
  ...jaCore,
  ...jaSettings,
  ...jaVoice,
  chat: { ...jaCore.chat, diagnosis: jaDiagnosis },
} as const;
