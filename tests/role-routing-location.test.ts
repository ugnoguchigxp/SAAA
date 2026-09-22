import { expect, test } from "bun:test";
import fixtures from "./fixtures/ipc-receivers.json";

test("rr_ls_30 ipc routing snapshot exposes the location fallback fields", () => {
  expect(fixtures.routingRoot.selectedRecipeId).toBe("20-respond-away");
  expect(fixtures.routingRoot.decisionReasonCodes).toEqual(["rules", "location_fallback"]);
});
