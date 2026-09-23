import { describe, expect, test } from "bun:test";
import { readFileSync } from "node:fs";
import { join } from "node:path";

function source(path: string): string {
  return readFileSync(join(import.meta.dir, "..", path), "utf8");
}

describe("top-level workspace redesign", () => {
  test("uses the agreed left-aligned routes and keeps Artifact outside the active page", () => {
    const routes = source("src/shell/appRoute.ts");
    const shell = source("src/shell/AppShell.tsx");
    const shellStyles = source("src/shell/appShell.css");
    const app = source("src/App.tsx");
    const chat = source("src/features/chat/ChatPage.tsx");

    expect(routes).toContain(
      '[\n  "conversation",\n  "memory",\n  "work",\n  "records",\n  "audit",\n  "diagnosis",\n  "settings",\n]',
    );
    expect(shell).toContain("<TopNavigation");
    expect(shellStyles).toContain("position: absolute");
    expect(shellStyles).toContain("pointer-events: none");
    expect(shellStyles).toContain(".workspace-page-toolbar");
    expect(shellStyles).toContain("top: 8px");
    expect(
      shellStyles.slice(
        shellStyles.indexOf(".app-primary-region"),
        shellStyles.indexOf(".top-navigation"),
      ),
    ).not.toContain("grid-template-rows");
    expect(app.indexOf("<ArtifactWorkspaceProvider>")).toBeLessThan(app.indexOf("<AppShell"));
    expect(chat).not.toContain("ChatOverflowMenu");
    expect(chat).not.toContain("WorldScopeSelector");
    expect(chat).not.toContain('className="topbar"');
    expect(chat).not.toContain("<CodingJobs");
    expect(chat).toContain('className="latest-message-button"');
    expect(chat).toContain('name="down"');
    expect(chat).toContain('className="llm-thinking-indicator"');
    expect(chat).toContain("voice-activity-indicator");
    expect(chat).not.toContain("transcript-stable");
  });

  test("connects each new page to its existing source of truth", () => {
    const memory = source("src/features/memory/MemoryPage.tsx");
    const work = source("src/features/work/WorkPage.tsx");
    const records = source("src/features/records/RecordsPage.tsx");
    const settings = source("src/features/settings/PersonalStateSection.tsx");

    expect(memory).toContain('from "@tanstack/react-table"');
    expect(memory).toContain("personalStateApi.snapshot()");
    expect(memory).toContain("personalStateApi.forget(sourceId)");
    expect(work).toContain("codingApi.settings()");
    expect(work).toContain("stewardApi.reorderQueue");
    expect(work).toContain("workApi.artifacts()");
    expect(records).toContain('from "@tanstack/react-table"');
    expect(records).toContain('className="records-dates"');
    expect(records).toContain('className="records-table"');
    expect(records).toContain("listMessages(id, cursor)");
    expect(settings).not.toContain("snapshot.items.map");
    expect(memory).not.toContain("memory-page-title");
    expect(work).not.toContain("work-page-title");
    expect(records).not.toContain("records-page-title");
    expect(source("src/features/audit/AuditLogPage.tsx")).not.toContain("audit-log-title");
  });
});
