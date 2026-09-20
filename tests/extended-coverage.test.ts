import { describe, expect, test } from "bun:test";
import { renderSafeMarkdown } from "../src/features/chat/markdownRenderer";
import { productionLines } from "../scripts/module-size";
import {
  availableTimeZones,
  isSupportedTimeZone,
  systemTimeZone,
} from "../src/lib/regionalPreferences";
import {
  beginRunPerformance,
  recordFirstDelta,
  recordMarkdownPaint,
  recordPlainCommit,
  recordRunWithoutMarkdown,
  recordSocketReceive,
  runPerformanceSnapshot,
} from "../src/features/chat/streamingPerformance";
import i18n from "../src/i18n";
import {
  formatRegionalDateTime,
  localizeForegroundCategory,
  localizeProviderKind,
  localizeProviderLabel,
  localizeRuntimeActivity,
  localizeSituationAttention,
  localizeSituationEntryKind,
  localizeSituationReason,
  localizeUiMessage,
} from "../src/i18n/presentation";

describe("extended coverage helpers", () => {
  test("renders lists, quotes, emphasis, and nested inline markup", () => {
    const html = renderSafeMarkdown(
      [
        "- one",
        "- two",
        "",
        "1. first",
        "2. second",
        "",
        "> quoted *em* and ~~del~~",
        "",
        "A paragraph with `code` and **bold**.",
      ].join("\n"),
    );
    expect(html).toContain("<ul>");
    expect(html).toContain("<ol>");
    expect(html).toContain("<blockquote>");
    expect(html).toContain("<em>em</em>");
    expect(html).toContain("<del>del</del>");
    expect(html).toContain("<code>code</code>");
  });

  test("counts production lines around comments, strings, and character literals", () => {
    const source = [
      "fn production() {",
      "  // {",
      "  /* { */",
      "}",
      "",
      "#[cfg(test)]",
      "mod tests { fn works() {} }",
      "",
    ].join("\n");
    expect(productionLines(source, "src/example.rs")).toBe(5);
  });

  test("lists supported time zones and rejects malformed identifiers", () => {
    expect(isSupportedTimeZone("system")).toBe(true);
    expect(isSupportedTimeZone("UTC")).toBe(true);
    expect(isSupportedTimeZone("not a zone")).toBe(false);
    expect(availableTimeZones()).toContain(systemTimeZone());
  });

  test("ignores duplicate socket marks and records cancelled runs", () => {
    beginRunPerformance("run_perf_cancel");
    recordSocketReceive("run_perf_cancel");
    recordSocketReceive("run_perf_cancel");
    recordFirstDelta("missing");
    recordRunWithoutMarkdown("run_perf_cancel", "cancelled");
    recordMarkdownPaint("missing-message");
    expect(runPerformanceSnapshot("run_perf_cancel")?.terminal).toBe("cancelled");
    expect(runPerformanceSnapshot("missing")).toBeNull();
    expect(recordPlainCommit("missing")).toBe(false);
    for (let index = 0; index < 70; index += 1) beginRunPerformance(`overflow-${index}`);
    expect(runPerformanceSnapshot("run_perf_cancel")).toBeNull();
  });

  test("maps remaining situation and provider labels", async () => {
    await i18n.changeLanguage("en");
    expect(localizeForegroundCategory(i18n.t, "coding")).toBe("Coding app");
    expect(localizeForegroundCategory(i18n.t, "mystery")).toBe("Unknown");
    expect(localizeSituationReason(i18n.t, "unknown")).not.toBe("");
    expect(localizeSituationAttention(i18n.t, "busy")).not.toBe("");
    expect(localizeSituationAttention(i18n.t, "other")).toBe("Unknown");
    expect(localizeSituationEntryKind(i18n.t, "other")).toBe("Unknown");
    expect(localizeProviderKind(i18n.t, "larm")).not.toBe("");
    expect(localizeProviderKind(i18n.t, "other")).toBe("Unknown");
    expect(localizeProviderLabel(i18n.t, "Model not selected")).not.toBe("Model not selected");
    expect(localizeProviderLabel(i18n.t, "Cloud LLM")).toContain("LLM");
    expect(localizeProviderLabel(i18n.t, "Custom")).toBe("Custom");
    expect(localizeRuntimeActivity(i18n.t, { type: "providerWorking" })).not.toBe("");
    expect(localizeRuntimeActivity(i18n.t, { type: "generationCancelled" })).not.toBe("");
    expect(localizeRuntimeActivity(i18n.t, { type: "voiceQueryQueued" })).not.toBe("");
    expect(localizeUiMessage(i18n.t, "ASR_NO_SPEECH", "voice")).not.toBe("ASR_NO_SPEECH");
    expect(formatRegionalDateTime("not-a-date", "en-US", "UTC")).toBe("not-a-date");
  });
});
