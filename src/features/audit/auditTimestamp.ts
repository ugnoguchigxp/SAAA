import type { AuditEvent } from "../../lib/contracts";

function parseAuditTimestamp(value: AuditEvent["occurredAt"]): Date | null {
  const milliseconds = Number(value);
  const date = Number.isFinite(milliseconds) ? new Date(milliseconds) : new Date(value);
  return Number.isNaN(date.getTime()) ? null : date;
}

export function auditTimestampIso(value: AuditEvent["occurredAt"]): string {
  return parseAuditTimestamp(value)?.toISOString() ?? value;
}

export function formatAuditTimestamp(value: AuditEvent["occurredAt"], locale: string): string {
  const date = parseAuditTimestamp(value);
  if (!date) {
    return value;
  }
  return new Intl.DateTimeFormat(locale, {
    year: "numeric",
    month: "2-digit",
    day: "2-digit",
    hour: "2-digit",
    minute: "2-digit",
    second: "2-digit",
  }).format(date);
}
