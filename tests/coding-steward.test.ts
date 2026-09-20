import { expect, test } from "bun:test";
import "./tauriCoreMock";
import { stewardErrorMessage } from "../src/features/coding/stewardApi";

test("duplicate steward goals are explained without a new IPC", () => {
  expect(stewardErrorMessage("active_goal_exists")).toContain("有効な Goal");
  expect(stewardErrorMessage("steward_register_invalid")).toContain("成功条件");
});
