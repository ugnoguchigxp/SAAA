import { describe, expect, test } from "bun:test";
import { readFileSync } from "node:fs";
import { join } from "node:path";

function source(path: string): string {
  return readFileSync(join(import.meta.dir, "..", path), "utf8");
}

describe("audit log UI", () => {
  test("uses a bounded read-only command from a top-level surface", () => {
    const backend = source("src-tauri/src/lib.rs");
    const audit = source("src-tauri/src/persistence/audit.rs");
    const app = source("src/App.tsx");
    const settings = source("src/features/settings/SettingsPage.tsx");
    const page = source("src/features/audit/AuditLogPage.tsx");
    const styles = source("src/features/audit/AuditLogPage.css");
    const packageJson = source("package.json");
    const runtime = source("src/lib/runtime.ts");

    expect(audit).toContain("fn list_audit_events");
    expect(backend).toContain("persistence::audit::list_audit_events,");
    expect(audit).toContain("const AUDIT_UI_EVENT_LIMIT: usize = 200;");
    expect(app).toContain('<AppIcon name="audit" />{t("app.audit")}');
    expect(app).toContain('surface === "audit" ? <AuditLogPage />');
    expect(settings).not.toContain("AuditLog");
    expect(runtime).toContain('invoke<AuditEvent[]>("list_audit_events")');
    expect(packageJson).toContain('"@tanstack/react-table"');
    expect(page).toContain('from "@tanstack/react-table"');
    expect(page).toContain("table.getHeaderGroups()");
    expect(page).toContain("table.getRowModel().rows");
    expect(page).toContain("setSelectedEvent(row.original)");
    expect(page).toContain('role="dialog"');
    expect(page).toContain("selectedEvent.attributes");
    expect(styles).toContain(".audit-table-scroll::-webkit-scrollbar");
    expect(styles).toContain(".audit-drawer-body::-webkit-scrollbar");
    expect(page).not.toContain("deleteAudit");
    expect(page).not.toContain("updateAudit");
  });
});
