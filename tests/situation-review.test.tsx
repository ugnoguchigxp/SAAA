import { afterEach, beforeEach, describe, expect, test } from "bun:test";
import { act } from "react";
import type { Root } from "react-dom/client";
import type {
  CalibrationProfile,
  CalibrationRun,
  SituationReviewSnapshot,
} from "../src/lib/contracts";
import { invokeImpl, resetTauriCoreMock } from "./tauriCoreMock";
import { installJsdom } from "./jsdomGlobals";

await import("../src/i18n");
const { decodeReplayMetrics, SituationReview } =
  await import("../src/features/situation/review/SituationReview");

function profile(status: CalibrationProfile["status"], id = "profile-1"): CalibrationProfile {
  return {
    id,
    ruleVersion: "situation-v1",
    baseRuleVersion: null,
    status,
    parameters: {
      classificationMinConfidence: 70,
      lowConfidenceMax: 45,
      enterSampleCount: 3,
      exitSampleCount: 5,
      cooldownMs: 10_000,
      inputActiveMaxMs: 30_000,
      inputRecentMaxMs: 300_000,
    },
    createdAt: "1",
    decidedAt: null,
    decisionReasonCode: null,
  };
}

function reviewSnapshot(): SituationReviewSnapshot {
  const run: CalibrationRun = {
    id: "run-1",
    profileId: "profile-1",
    fixtureSetVersion: "v1",
    status: "completed",
    metricsJson: JSON.stringify({
      fixtureSetVersion: "v1",
      sampleCount: 10,
      expectedSceneMatches: 8,
      baselineExpectedSceneMatches: 7,
      expectedAttentionSamples: 4,
      expectedAttentionMatches: 3,
      baselineExpectedAttentionMatches: 2,
      shadowPolicyCounts: { keep: 1 },
    }),
    errorCode: null,
    startedAt: "1",
    completedAt: "2",
  };
  return {
    activeProfile: profile("active"),
    quality: { sampleCount: 10, flappingRate: 0.1, staleRate: null },
    feedbackQueue: [],
    latestRun: run,
    candidates: [profile("candidate"), profile("superseded", "profile-2")],
  };
}

describe("situation review", () => {
  let root: Root | null = null;
  let restore: (() => void) | null = null;
  let snapshot = reviewSnapshot();
  let failLoad = false;

  beforeEach(() => {
    snapshot = reviewSnapshot();
    failLoad = false;
    resetTauriCoreMock();
    invokeImpl.handler = async (command) => {
      if (failLoad && command === "get_situation_review_snapshot") throw new Error("offline");
      if (command === "get_situation_review_snapshot" || command === "decide_situation_calibration")
        return snapshot;
      if (command === "create_situation_calibration_candidate") return snapshot.candidates[0];
      if (command === "run_situation_calibration") return snapshot.latestRun;
      return command;
    };
  });

  afterEach(async () => {
    await act(async () => root?.unmount());
    root = null;
    restore?.();
    restore = null;
  });

  test("decodes replay metrics and rejects malformed payloads", () => {
    expect(decodeReplayMetrics(null)).toBeNull();
    expect(decodeReplayMetrics("{")).toBeNull();
    expect(decodeReplayMetrics(JSON.stringify({ fixtureSetVersion: "v1" }))).toBeNull();
    expect(decodeReplayMetrics(snapshot.latestRun?.metricsJson ?? null)?.fixtureSetVersion).toBe(
      "v1",
    );
  });

  test("loads a snapshot and runs candidate actions", async () => {
    const env = installJsdom();
    restore = env.restore;
    env.dom.window.confirm = () => true;
    const { createRoot } = await import("react-dom/client");
    const { createElement } = await import("react");
    root = createRoot(document.getElementById("root")!);
    await act(async () => root!.render(createElement(SituationReview)));
    await act(async () => {
      await Promise.resolve();
    });
    expect(document.body.textContent).toContain("situation-v1");
    const buttons = [...document.querySelectorAll("button")];
    await act(async () =>
      buttons.find((button) => button.textContent?.includes("Replay"))?.click(),
    );
    await act(async () =>
      buttons.find((button) => button.textContent?.includes("Create"))?.click(),
    );
    await act(async () =>
      buttons.find((button) => button.textContent?.includes("Accept"))?.click(),
    );
    await act(async () =>
      buttons.find((button) => button.textContent?.includes("Rollback"))?.click(),
    );
    const number = document.querySelector<HTMLInputElement>("input[type='number']");
    if (number) {
      await act(async () => {
        const setter = Object.getOwnPropertyDescriptor(
          env.dom.window.HTMLInputElement.prototype,
          "value",
        )!.set!;
        setter.call(number, "80");
        number.dispatchEvent(new env.dom.window.Event("input", { bubbles: true }));
      });
    }
    expect(document.querySelector(".error-banner")).toBeNull();
  });

  test("shows a localized load error", async () => {
    failLoad = true;
    const env = installJsdom();
    restore = env.restore;
    const { createRoot } = await import("react-dom/client");
    const { createElement } = await import("react");
    root = createRoot(document.getElementById("root")!);
    await act(async () => root!.render(createElement(SituationReview)));
    await act(async () => {
      await Promise.resolve();
    });
    expect(document.querySelector(".error-banner")).not.toBeNull();
  });
});
