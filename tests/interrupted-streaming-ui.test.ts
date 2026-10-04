import { expect, test } from "bun:test";
import { readFileSync } from "node:fs";

test("current queue clears uncommitted output after refusal or cancellation", () => {
  const page = readFileSync(
    new URL("../src/features/chat/ConversationCheckPage.tsx", import.meta.url),
    "utf8",
  );
  expect(page).toContain("message.id === `reply_${inputId}`");
  expect(page).toContain('["failed", "cancelled"].includes(job.state)');
  expect(page).toContain('latestJob?.state === "failed" ? latestJob.error : null');
  expect(page).toContain("この依頼を中止");
});
