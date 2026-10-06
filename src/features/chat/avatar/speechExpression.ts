import type { AvatarCue } from "./LightAvatarBackground";
import type { AvatarMotion } from "./model.js";

export const avatarMotions: readonly AvatarMotion[] = [
  "neutral",
  "listening",
  "thinking",
  "speaking",
  "curious",
  "distant",
  "downcast",
  "greeting",
  "agreeing",
  "joyful",
  "surprised",
  "shy",
  "sleepy",
];

type PlaybackExpression = {
  cue: AvatarCue | null;
  error: string | null;
  chunkId: number;
  ended: boolean;
};
export const restingExpression: PlaybackExpression = {
  cue: null,
  error: null,
  chunkId: 0,
  ended: true,
};

export function speechExpression(
  previous: PlaybackExpression,
  value: unknown,
  conversationId: string,
): PlaybackExpression {
  if (!value || typeof value !== "object") return previous;
  const event = value as Record<string, unknown>;
  if (
    event.conversationId !== conversationId ||
    typeof event.chunkId !== "number" ||
    !Number.isSafeInteger(event.chunkId) ||
    event.chunkId <= 0 ||
    !avatarMotions.includes(event.motion as AvatarMotion) ||
    typeof event.fallback !== "boolean" ||
    (event.phase !== "started" && event.phase !== "ended")
  )
    return previous;
  if (
    event.chunkId < previous.chunkId ||
    (event.chunkId === previous.chunkId && (previous.ended || event.phase === "started"))
  )
    return previous;
  const ended = event.phase === "ended";
  return {
    chunkId: event.chunkId,
    ended,
    cue: {
      id: `${event.chunkId}:${event.phase}`,
      motion: ended ? "neutral" : (event.motion as AvatarMotion),
    },
    error: !ended && event.fallback ? "この発話は通常の表情と声で読み上げています。" : null,
  };
}
