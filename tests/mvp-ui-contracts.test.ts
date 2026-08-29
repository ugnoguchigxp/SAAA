import { describe, expect, test } from "bun:test";
import { readFileSync } from "node:fs";
import { join } from "node:path";

function source(path: string): string {
  return readFileSync(join(import.meta.dir, "..", path), "utf8");
}

describe("MVP UI reachability contracts", () => {
  test("keeps Coding assist reachable with an explicit read-only workspace", () => {
    const app = source("src/App.tsx");
    expect(app).toContain('switchTaskMode("coding")');
    expect(app).toContain("chooseWorkspace()");
    expect(app).toContain('runMode === "coding" ? workspacePath.trim() || null : null');
    expect(app).toContain("Codex ready");
  });

  test("keeps Codex configuration and coding routing visible in Settings", () => {
    const settings = source("src/features/settings/SettingsPage.tsx");
    expect(settings).toContain('id: "codex"');
    expect(settings).toContain("coding.assist");
    expect(settings).toContain("Safety policy");
  });

  test("renders actual partial and final transcript events", () => {
    const contracts = source("src/lib/contracts.ts");
    const meeting = source("src/features/meeting/useMeetingSession.ts");
    expect(contracts).toContain('type: "transcriptDelta"');
    expect(contracts).toContain('type: "transcriptPartial"');
    expect(meeting).toContain('event.type === "transcriptPartial" || event.type === "transcriptFinal"');
  });
});
