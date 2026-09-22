import { enCore } from "./enCore";
import { enSettings } from "./enSettings";
import { enVoice } from "./enVoice";
export const en = {
  ...enCore,
  ...enSettings,
  ...enVoice,
} as const;
