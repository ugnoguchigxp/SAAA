import { expect, mock, test } from "bun:test";
import { act } from "react";
import { installJsdom } from "./jsdomGlobals";
let callback: ((event: { payload: unknown }) => void) | undefined;
let stops = 0;
mock.module("@tauri-apps/api/event", () => ({
  listen: async (_name: string, next: typeof callback) => {
    callback = next;
    return () => stops++;
  },
}));
const { avatarMotions, restingExpression, speechExpression, useAvatarDecision } =
  await import("../src/features/chat/avatar/useAvatarDecision");
import question from "../src-tauri/src/providers/laya/avatar-question.json";
const event = (chunkId: number, motion = "joyful", phase = "started") => ({
  conversationId: "conversation",
  chunkId,
  motion,
  phase,
  fallback: false,
});
test("each TTS chunk updates the pose; duplicate, stale, foreign and invalid events cannot override it", () => {
  expect(Object.keys(question.motion.criteria).sort()).toEqual([...avatarMotions].sort());
  const first = speechExpression(restingExpression, event(1), "conversation");
  expect(first.cue?.motion).toBe("joyful");
  const second = speechExpression(first, event(2, "downcast"), "conversation");
  expect(second.cue?.motion).toBe("downcast");
  for (const value of [
    null,
    {},
    event(1),
    event(1, "joyful", "ended"),
    event(2),
    event(3, "invalid"),
    { ...event(3), conversationId: "other" },
  ])
    expect(speechExpression(second, value, "conversation")).toBe(second);
  const ended = speechExpression(second, event(2, "downcast", "ended"), "conversation");
  expect(ended.cue?.motion).toBe("neutral");
  expect(speechExpression(ended, event(2), "conversation")).toBe(ended);
  expect(
    speechExpression(ended, { ...event(3, "neutral"), fallback: true }, "conversation").error,
  ).toBeTruthy();
});
test("hidden/unmounted avatars unsubscribe and ignore later speech events", async () => {
  const env = installJsdom();
  const { createRoot } = await import("react-dom/client");
  const root = createRoot(document.getElementById("root")!);
  function Probe({ active = true }: { active?: boolean }) {
    const state = useAvatarDecision("conversation", active);
    return <p>{state.cue?.motion ?? "neutral"}</p>;
  }
  try {
    await act(async () => root.render(<Probe />));
    await act(async () => callback!({ payload: event(1) }));
    expect(document.querySelector("p")?.textContent).toBe("joyful");
    const old = callback!;
    await act(async () => root.render(<Probe active={false} />));
    expect(stops).toBe(1);
    await act(async () => old({ payload: event(2) }));
    expect(document.querySelector("p")?.textContent).toBe("neutral");
    await act(async () => root.render(<Probe />));
    await act(async () => callback!({ payload: event(3, "thinking") }));
    expect(document.querySelector("p")?.textContent).toBe("thinking");
  } finally {
    await act(async () => root.unmount());
    expect(stops).toBe(2);
    env.restore();
  }
});
