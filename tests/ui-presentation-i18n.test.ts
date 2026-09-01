import { afterEach, describe, expect, test } from "bun:test";
import i18n from "../src/i18n";
import {
  formatRegionalDateTime,
  localizeRuntimeActivity,
  localizeSituationEntryKind,
  localizeSituationScene,
  localizeStatus,
  localizeUiMessage,
  uiMessage,
} from "../src/i18n/presentation";
import { appendConversationActivity } from "../src/lib/conversationActivity";

afterEach(() => void i18n.changeLanguage("en"));

describe("localized UI presentation", () => {
  test("renders stable UI error identities in the selected language", async () => {
    await i18n.changeLanguage("ja");

    expect(localizeUiMessage(i18n.t, uiMessage("chatVoiceQueueFull"), "chat"))
      .toBe("音声処理が混み合っているため、最新の発話は送信しませんでした。");
    expect(localizeUiMessage(i18n.t, "ASR_LANGUAGE_UNKNOWN: fixture", "voice"))
      .toBe("使用言語を判定できなかったため、発話を送信しませんでした。");
    expect(localizeUiMessage(
      i18n.t,
      "LARM_API_TOKEN is invalid.",
      "settings",
    )).toBe("設定済みのLARM_API_TOKENが無効か、Agent Connection側で拒否されました。認証なしで使う場合は環境変数を削除してください。");
    expect(localizeUiMessage(
      i18n.t,
      "dynamic_lan rejected the connection authorization.",
      "settings",
    )).toBe("Agent Connectionが接続を拒否しました。ローカルLANの匿名アクセスを許可するか、正しいLARM_API_TOKENを設定してください。");
  });

  test("does not expose untrusted backend error text in either language", async () => {
    await i18n.changeLanguage("en");
    const message = localizeUiMessage(i18n.t, "provider returned a private diagnostic", "settings");

    expect(message).toBe("The setting could not be updated. Check the values and try again.");
    expect(message).not.toContain("private diagnostic");
  });

  test("maps runtime states, typed activities, and Situation identifiers to display labels", async () => {
    await i18n.changeLanguage("ja");

    expect(localizeStatus(i18n.t, "saaa-transcribing")).toBe("SAAAが文字起こし中");
    expect(localizeRuntimeActivity(i18n.t, { type: "providerStarted", providerId: "provider-a" })).toBe("provider-a を使用中");
    expect(localizeRuntimeActivity(i18n.t, { type: "providerSelected", providerId: "provider-b", fallbackUsed: true })).toBe("フォールバックプロバイダー provider-b を使用中");
    expect(localizeRuntimeActivity(i18n.t, { type: "providerFailed" })).toBe("プロバイダーがリクエストを完了できませんでした。");
    expect(localizeSituationScene(i18n.t, "CODING")).toBe("コーディング");
    expect(localizeSituationEntryKind(i18n.t, "heartbeat")).toBe("ハートビート");
  });

  test("keeps only bounded, structured runtime activities", () => {
    const activities = Array.from({ length: 9 }, (_, index) => ({
      type: "providerStarted" as const,
      providerId: `provider-${index}`,
    })).reduce(appendConversationActivity, []);

    expect(activities).toHaveLength(8);
    expect(activities[0]).toEqual({ type: "providerStarted", providerId: "provider-1" });
  });

  test("formats timestamps in the saved time zone", () => {
    const value = "0";
    const expected = new Date(0).toLocaleString("en-US", { timeZone: "Asia/Tokyo" });

    expect(formatRegionalDateTime(value, "en-US", "Asia/Tokyo")).toBe(expected);
    expect(formatRegionalDateTime(value, "en-US", "Asia/Tokyo"))
      .not.toBe(formatRegionalDateTime(value, "en-US", "UTC"));
  });
});
