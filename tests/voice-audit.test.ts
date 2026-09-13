import { describe, expect, test } from "bun:test";
import { invokeCalls, resetTauriCoreMock } from "./tauriCoreMock";
import { MicrophoneCaptureError } from "../src/lib/microphone";

const {
  auditCaptureCancelled,
  auditCaptureFailed,
  auditCaptureStarted,
  auditCaptureSuspended,
  auditVoiceDeliveryBlocked,
  auditVoiceDeliveryDecision,
  auditVoiceDeliverySettlement,
} = await import("../src/features/voice/voiceAudit");

describe("voice audit projections", () => {
  test("records capture and delivery decisions with bounded failure codes", () => {
    resetTauriCoreMock();
    auditCaptureStarted("s1", "c1", "recording");
    auditCaptureCancelled("s1", "c1");
    auditCaptureFailed("s1", "c1", new MicrophoneCaptureError("permission-denied", "denied"));
    auditCaptureFailed("s1", "c1", "asr-provider-unavailable");
    auditCaptureFailed("s1", "c1", "other");
    auditCaptureSuspended("s1", "c1", "meeting");
    auditVoiceDeliveryBlocked("s1", "u1", "c1", 2);
    auditVoiceDeliveryBlocked("s1", "u1", "c1");
    const utterance = { sessionId: "s1", utteranceId: "u1", conversationId: "c1", text: "hello" };
    auditVoiceDeliveryDecision(utterance, "queued", 1);
    auditVoiceDeliveryDecision(utterance, "immediate");
    let settled: boolean | undefined;
    const settle = auditVoiceDeliverySettlement(utterance, (delivered) => {
      settled = delivered;
    });
    settle(true);
    settle(false);
    expect(settled).toBe(false);
    const names = invokeCalls.map(
      (call) => (call.args as { input?: { eventName?: string; failureCode?: string } })?.input,
    );
    expect(names.some((event) => event?.eventName === "capture-started")).toBe(true);
    expect(names.some((event) => event?.failureCode === "permission-denied")).toBe(true);
    expect(names.some((event) => event?.failureCode === "asr-provider-unavailable")).toBe(true);
    expect(names.some((event) => event?.failureCode === "unknown")).toBe(true);
  });
});
