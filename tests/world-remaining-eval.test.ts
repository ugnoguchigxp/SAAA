import { expect, test } from "bun:test";
import { parseTests, validateTests } from "../scripts/world-remaining-eval";
test("wr_t22_missing_duplicate_failed_and_zero_tests_fail", () => {
  expect(validateTests([], ["wr_t01_"])).toContain("zero tests");
  const cases = parseTests(
    "test module::wr_t01_contract ... ok\ntest module::wr_t02_scope ... FAILED\n",
    "fixture",
  );
  expect(validateTests(cases, ["wr_t03_"]).length).toBe(2);
  expect(validateTests([cases[0], cases[0]], [])).toContain("duplicate test identity");
  expect(validateTests([cases[0]], ["wr_t01_"])).toEqual([]);
  const fake = [{ ...cases[0], name: "module::not_wr_t01_fake" }];
  expect(validateTests(fake, ["wr_t01_"])).toContain("missing passing card wr_t01_");
});
