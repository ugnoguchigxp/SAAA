import { jaCore } from "./jaCore";
import { jaSettings } from "./jaSettings";
import { jaVoice } from "./jaVoice";
export const ja = {
  ...jaCore,
  ...jaSettings,
  ...jaVoice,
} as const;
