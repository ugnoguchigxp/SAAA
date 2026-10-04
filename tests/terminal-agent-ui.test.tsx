import { expect, mock, test } from "bun:test";
import { act } from "react";
import { createRoot } from "react-dom/client";
import { installJsdom } from "./jsdomGlobals";
import type { CodingSettings } from "../src/lib/generated/coding";
const calls: { command: string; args: Record<string, unknown> }[] = [];
const settings: CodingSettings = {
  enabled: true,
  implementationMethod: "pi",
  codexModel: "existing-codex",
  executable: "/existing/pi",
  version: "0.86.1",
  provider: "existing-provider",
  model: "existing-model",
  profile: "trusted-local-v1",
  sdkExtensionPath: null,
  terminalKind: "",
  terminalCli: "",
  terminalExecutable: "",
  terminalModel: "",
  terminalChecks: [],
  terminalAutoAnswer: false,
  terminalRetryLimit: 0,
};
mock.module("@tauri-apps/api/core", () => ({
  invoke: async (command: string, args: Record<string, unknown> = {}) => {
    calls.push({ command, args });
    if (command === "get_coding_settings") return { ...settings };
    return { accepted: true };
  },
}));
const { CodingSettingsSection } = await import("../src/features/coding/CodingSettingsSection");
const { TerminalQuestion } = await import("../src/features/coding/TerminalQuestion");

test("selecting a dedicated terminal and CLI preserves the existing implementation settings", async () => {
  const dom = installJsdom();
  const container = document.createElement("div");
  document.body.append(container);
  const root = createRoot(container);
  try {
    await act(async () => {
      root.render(<CodingSettingsSection />);
    });
    const choice = [...container.querySelectorAll<HTMLInputElement>('input[type="radio"]')].find(
      (input) => input.parentElement?.textContent?.includes("専用端末"),
    )!;
    await act(async () => {
      choice.click();
    });
    const selects = container.querySelectorAll<HTMLSelectElement>("select");
    const terminal = [...selects].find((select) =>
      [...select.options].some((option) => option.value === "ghostty"),
    )!;
    const cli = [...selects].find((select) =>
      [...select.options].some((option) => option.value === "claude"),
    )!;
    await act(async () => {
      terminal.value = "ghostty";
      terminal.dispatchEvent(new Event("change", { bubbles: true }));
    });
    await act(async () => {
      cli.value = "claude";
      cli.dispatchEvent(new Event("change", { bubbles: true }));
    });
    const save = [...container.querySelectorAll<HTMLButtonElement>("button")].find(
      (button) => button.textContent === "実装設定を保存",
    )!;
    await act(async () => {
      save.click();
    });
    const saved = calls.findLast((call) => call.command === "save_coding_settings")?.args
      .settings as CodingSettings;
    expect(saved.implementationMethod).toBe("terminal");
    expect(saved.terminalKind).toBe("ghostty");
    expect(saved.terminalCli).toBe("claude");
    expect(saved.provider).toBe("existing-provider");
    expect(saved.executable).toBe("/existing/pi");
    expect(saved.codexModel).toBe("existing-codex");
  } finally {
    await act(async () => root.unmount());
    dom.restore();
  }
});

test("pending questions cannot be answered until exit; paused permissions send one scoped approval", async () => {
  const dom = installJsdom();
  const container = document.createElement("div");
  document.body.append(container);
  const root = createRoot(container);
  let saved = 0;
  const question = {
    questionId: "run:question",
    kind: "permission",
    state: "pending",
    input: { tool_name: "Bash", input: { command: "bun test" } },
  };
  const props = {
    question,
    conversationId: "conversation",
    jobId: "job",
    revision: 9,
    onSaved: () => saved++,
    onError: () => {},
  };
  try {
    await act(async () => root.render(<TerminalQuestion {...props} />));
    expect(container.querySelector<HTMLButtonElement>("button")?.disabled).toBe(true);
    await act(async () =>
      root.render(
        <TerminalQuestion {...props} question={{ ...question, state: "awaiting_user" }} />,
      ),
    );
    await act(async () => {
      container.querySelector<HTMLButtonElement>("button")!.click();
    });
    const sent = calls.findLast((call) => call.command === "answer_terminal_question")!;
    expect(sent.args).toEqual({
      conversationId: "conversation",
      jobId: "job",
      expectedRevision: 9,
      questionId: "run:question",
      answer: "approve",
    });
    expect(saved).toBe(1);
  } finally {
    await act(async () => root.unmount());
    dom.restore();
  }
});
