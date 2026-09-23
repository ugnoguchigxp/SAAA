import { containsSource } from "./sourceContract";
import { describe, expect, test } from "bun:test";
import { readFileSync } from "node:fs";
import { join } from "node:path";

function source(path: string): string {
  return readFileSync(join(import.meta.dir, "..", path), "utf8");
}

describe("audit log UI", () => {
  test("keeps a bounded read-only audit command and page", () => {
    const backend = source("src-tauri/src/runtime/command_registry.rs");
    const audit = [
      source("src-tauri/src/persistence/audit/record_event.rs"),
      source("src-tauri/src/persistence/audit/audit_event_sort_field.rs"),
    ].join("\n");
    const app = source("src/App.tsx");
    const settings = source("src/features/settings/SettingsPage.tsx");
    const page = source("src/features/audit/AuditLogPage.tsx");
    const styles = source("src/features/audit/AuditLogPage.css");
    const packageJson = source("package.json");
    const runtime = source("src/lib/runtime.ts");

    expect(containsSource(audit, "fn list_audit_events")).toBe(true);
    expect(containsSource(backend, "persistence::audit::record_event::list_audit_events,")).toBe(true);
    expect(containsSource(audit, "const AUDIT_UI_EVENT_LIMIT: usize = 200;")).toBe(true);
    expect(containsSource(app, "AuditLogPage")).toBe(true);
    expect(containsSource(app, 'route === "audit"')).toBe(true);
    expect(containsSource(page, "useQuery({")).toBe(true);
    expect(containsSource(page, 'queryKey: ["audit-events", sortBy, direction]')).toBe(true);
    expect(containsSource(page, "manualSorting: true")).toBe(true);
    expect(containsSource(page, "getToggleSortingHandler()")).toBe(true);
    expect(containsSource(page, 'selectedEvent.outcome === "failure"')).toBe(true);
    expect(containsSource(page, "buildAuditDebugContext(event)")).toBe(true);
    expect(containsSource(page, "navigator.clipboard?.writeText")).toBe(true);
    expect(containsSource(settings, "AuditLog")).toBe(false);
    expect(containsSource(runtime, 'invoke<AuditEvent[]>("list_audit_events", { input })')).toBe(
      true,
    );
    expect(containsSource(packageJson, '"@tanstack/react-query"')).toBe(true);
    expect(containsSource(packageJson, '"@tanstack/react-table"')).toBe(true);
    expect(containsSource(page, 'from "@tanstack/react-table"')).toBe(true);
    expect(containsSource(page, "table.getHeaderGroups()")).toBe(true);
    expect(containsSource(page, "table.getRowModel().rows")).toBe(true);
    expect(containsSource(page, "setSelectedEventId(row.original.id)")).toBe(true);
    expect(containsSource(page, 'role="dialog"')).toBe(true);
    expect(containsSource(page, "selectedEvent.attributes")).toBe(true);
    expect(containsSource(styles, ".audit-table-scroll::-webkit-scrollbar")).toBe(true);
    expect(containsSource(styles, ".audit-drawer-body::-webkit-scrollbar")).toBe(true);
    expect(containsSource(page, "deleteAudit")).toBe(false);
    expect(containsSource(page, "updateAudit")).toBe(false);
  });
});
