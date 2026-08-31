import { useState, type Dispatch, type SetStateAction } from "react";

type ErrorSlot = "app" | "conversation" | "voice";
type ErrorSlots = Record<ErrorSlot, string | null>;

export function useAppErrors() {
  const [errors, setErrors] = useState<ErrorSlots>({ app: null, conversation: null, voice: null });
  const setter = (slot: ErrorSlot): Dispatch<SetStateAction<string | null>> => (value) => {
    setErrors((current) => ({
      ...current,
      [slot]: typeof value === "function" ? value(current[slot]) : value,
    }));
  };
  return {
    errors,
    error: errors.conversation ?? errors.voice ?? errors.app,
    setAppError: setter("app"),
    setConversationError: setter("conversation"),
    setVoiceError: setter("voice"),
  };
}
