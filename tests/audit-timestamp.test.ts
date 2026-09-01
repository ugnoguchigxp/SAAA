import { describe, expect, test } from "bun:test";
import { auditTimestampIso, formatAuditTimestamp } from "../src/features/audit/auditTimestamp";

describe("audit timestamp presentation", () => {
  test("parses the Unix millisecond strings returned by the audit command", () => {
    const occurredAt = "1756728000000";

    expect(auditTimestampIso(occurredAt)).toBe("2025-09-01T12:00:00.000Z");
    expect(formatAuditTimestamp(occurredAt, "en-US")).not.toBe(occurredAt);
  });

  test("keeps ISO timestamps compatible and does not throw on malformed data", () => {
    const isoTimestamp = "2025-09-01T12:00:00.000Z";

    expect(auditTimestampIso(isoTimestamp)).toBe(isoTimestamp);
    expect(auditTimestampIso("invalid")).toBe("invalid");
    expect(formatAuditTimestamp("invalid", "en-US")).toBe("invalid");
  });
});
