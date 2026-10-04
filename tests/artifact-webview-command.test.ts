import { expect, test } from "bun:test";
import {
  nextWebsiteTabIndex,
  planWebviewCommand,
} from "../src/features/chat/artifacts/artifactTab";

test("a single website tab stays selected when moving next", () => {
  expect(nextWebsiteTabIndex(0, 1, 1)).toBe(0);
  expect(nextWebsiteTabIndex(0, 2, 1)).toBe(1);
  expect(nextWebsiteTabIndex(0, 0, 1)).toBeNull();
  expect(planWebviewCommand("next_tab", 0, 1)).toEqual({ outcome: "ack-now" });
  expect(planWebviewCommand("next_tab", 0, 2)).toEqual({
    outcome: "reduce-then-ack",
    action: "select",
  });
  expect(planWebviewCommand("scroll", 0, 1)).toEqual({ outcome: "scroll-then-ack" });
  expect(planWebviewCommand("close_all_tabs", 0, 2)).toEqual({
    outcome: "reduce-then-ack",
    action: "close-all",
  });
});
