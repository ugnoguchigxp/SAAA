import { expect, test } from "bun:test";
import { endReasoningRun, isReasoningRun, markReasoningRun, markReasoningCancellation, reasoningCancellationRequested } from "../src/lib/reasoningRun";
test("only a live reasoning run opts into replacement on follow-up input", () => {
  expect(isReasoningRun(null)).toBe(false);
  expect(isReasoningRun("normal")).toBe(false);
  markReasoningRun("reasoning", "conversation");
  expect(isReasoningRun("reasoning")).toBe(true);
  markReasoningCancellation("reasoning");
  expect(reasoningCancellationRequested("reasoning")).toBe(true);
  endReasoningRun("reasoning");
  expect(reasoningCancellationRequested("reasoning")).toBe(false);
  expect(isReasoningRun("reasoning")).toBe(false);
});

import { queueReasoningReplacement } from "../src/lib/reasoningRun";
import type { PendingConversationPrompt } from "../src/lib/conversationSession";
test("follow-up preserves options and bounded queues report non-delivery", () => {
  const pending: PendingConversationPrompt[] = [];
  expect(queueReasoningReplacement("normal", "質問", pending, {}, "conversation")).toBeNull();
  markReasoningRun("replace", "conversation");
  expect(queueReasoningReplacement("replace", "条件の追加", pending, { inputOrigin: "voice", sourceId: "utterance_1" }, "conversation")).toBe("queued");
  expect(pending[0].sourceId).toBe("utterance_1");
  expect(queueReasoningReplacement("replace", "続き", pending, {}, "conversation")).toBe("queued");
  let delivered: boolean | undefined;
  expect(queueReasoningReplacement("replace", "3件目", pending, { onSettled: (value) => { delivered = value; } }, "conversation")).toBe("full");
  expect(delivered).toBe(false);
  expect(pending.length).toBe(2);
  endReasoningRun("replace");
});


test("input in a different conversation is never added to the old run's replacement queue", () => {
  const pending: PendingConversationPrompt[] = [];
  markReasoningRun("old-run", "old-conversation");
  expect(queueReasoningReplacement("old-run", "新しい会話への入力", pending, {}, "new-conversation")).toBeNull();
  expect(pending).toHaveLength(0);
  endReasoningRun("old-run");
});
