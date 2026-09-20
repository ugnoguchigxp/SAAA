import { describe, expect, test } from "bun:test";
import { readFileSync } from "node:fs";
import { join } from "node:path";

function source(path: string): string {
  return readFileSync(join(import.meta.dir, "../", path), "utf8");
}

describe("schedule settings", () => {
  test("schedule section stays default-off and omits tokens", () => {
    const ui = source("src/features/settings/ScheduleSection.tsx");
    expect(ui).toContain("enabled: false");
    expect(ui).toContain("lastError");
    expect(ui).not.toContain("refresh_token");
    expect(ui).not.toContain("access_token");
    const api = source("src/lib/scheduleApi.ts");
    expect(api).toContain("schedule_status");
  });
});
