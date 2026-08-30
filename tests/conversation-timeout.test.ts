import { describe, expect, test } from "bun:test";
import { defaultSettingsDraft } from "../src/features/settings/settingsDefaults";
import {
  conversationTimeoutMsFromSecondsInput,
  conversationTimeoutSecondsInputValue,
} from "../src/lib/conversationTimeout";
import { routingSettingsSchema } from "../src/lib/schemas";

describe("conversation timeout settings", () => {
  test("defaults the conversation LLM timeout to 1800 seconds", () => {
    expect(defaultSettingsDraft.routing.conversationRespond.timeoutMs).toBe(1_800_000);
  });

  test("accepts a configurable conversation LLM timeout up to one hour", () => {
    const routing = structuredClone(defaultSettingsDraft.routing);
    routing.conversationRespond.timeoutMs = 1_800_000;
    expect(routingSettingsSchema.safeParse(routing).success).toBe(true);
    routing.conversationRespond.timeoutMs = 3_600_001;
    expect(routingSettingsSchema.safeParse(routing).success).toBe(false);
  });

  test("converts bounded second inputs without losing millisecond-compatible values", () => {
    expect(conversationTimeoutMsFromSecondsInput("1800")).toBe(1_800_000);
    expect(conversationTimeoutMsFromSecondsInput("269.999")).toBe(269_999);
    expect(conversationTimeoutSecondsInputValue(269_999)).toBe("269.999");
    for (const invalid of ["", " 1800", "0.999", "3600.001", "1.0001", "Infinity", "text"]) {
      expect(conversationTimeoutMsFromSecondsInput(invalid)).toBeNull();
    }
  });
});
