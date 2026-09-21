import { describe, expect, test } from "bun:test";
import type { AuditEvent } from "../src/lib/contracts";
import { buildAuditDebugContext } from "../src/features/audit/AuditLogPage";

const failureEvent: AuditEvent = {
  sequence: 42,
  id: "audit_failure_42",
  occurredAt: "1789995600000",
  component: "provider",
  eventName: "runtime-run-finished",
  phase: "terminal",
  outcome: "failure",
  correlationId: "correlation-1",
  causationId: null,
  conversationId: "conversation-1",
  runtimeRunId: "run-1",
  sessionId: "session-1",
  subjectId: null,
  failureCode: "request-timeout",
  attributes: { providerKind: "openai-compatible", fallbackUsed: false },
};

describe("audit debug context", () => {
  test("formats bounded failure metadata for an LLM without inventing content", () => {
    const context = buildAuditDebugContext(failureEvent);

    expect(context).toContain("saaa.audit-failure-debug.v1");
    expect(context).toContain('"failureCode": "request-timeout"');
    expect(context).toContain('"runtimeRunId": "run-1"');
    expect(context).toContain('"fallbackUsed": false');
    expect(context).toContain("Treat missing fields as unknown");
    expect(context).toContain("message content, audio, and credentials are not included");
  });
});
