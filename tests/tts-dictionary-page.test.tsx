import { expect, test } from "bun:test";
import { act } from "react";
import { installJsdom } from "./jsdomGlobals";
import { invokeCalls, invokeImpl, resetTauriCoreMock } from "./tauriCoreMock";

test("edits a reading, previews it with the configured TTS, and persists it", async () => {
  const environment = installJsdom();
  resetTauriCoreMock();
  const { TtsDictionaryPage } = await import("../src/features/ttsDictionary/TtsDictionaryPage");
  const { createRoot } = await import("react-dom/client");
  const root = createRoot(document.getElementById("root")!);
  let entries = [{ written: "SAAA", spoken: "サー" }];
  invokeImpl.handler = async (command, args) => {
    if (command === "list_tts_dictionary") return entries;
    if (command === "search_tts_dictionary_presets") return { total: 80205, entries: [] };
    if (command === "save_tts_dictionary_entry") {
      const input = (args as { input: { entry: { written: string; spoken: string } } }).input;
      entries = [input.entry];
      return entries;
    }
    if (command === "preview_tts_dictionary") return undefined;
    throw new Error(`unexpected command: ${command}`);
  };
  try {
    await act(async () => root.render(<TtsDictionaryPage />));
    await act(async () =>
      document.querySelector<HTMLButtonElement>(".tts-dictionary-list > button")!.click(),
    );
    const reading = document.querySelector<HTMLInputElement>(
      ".tts-dictionary-editor label:nth-of-type(2) input",
    )!;
    await act(async () => {
      Object.getOwnPropertyDescriptor(window.HTMLInputElement.prototype, "value")?.set?.call(
        reading,
        "サーアーアーエー",
      );
      reading.dispatchEvent(new Event("input", { bubbles: true }));
    });
    await act(async () =>
      document.querySelector<HTMLButtonElement>(".tts-dictionary-preview button")!.click(),
    );
    expect(invokeCalls.find(({ command }) => command === "preview_tts_dictionary")?.args).toEqual({
      input: {
        text: "SAAAに相談してみましょう。",
        original: "SAAA",
        entry: { written: "SAAA", spoken: "サーアーアーエー" },
      },
    });
    await act(async () =>
      document.querySelector<HTMLButtonElement>(".tts-dictionary-actions .primary")!.click(),
    );
    expect(entries).toEqual([{ written: "SAAA", spoken: "サーアーアーエー" }]);
  } finally {
    await act(async () => root.unmount());
    environment.restore();
    resetTauriCoreMock();
  }
});

test("saves a symbol as a skipped reading and previews the omission", async () => {
  const environment = installJsdom();
  resetTauriCoreMock();
  const { TtsDictionaryPage } = await import("../src/features/ttsDictionary/TtsDictionaryPage");
  const { createRoot } = await import("react-dom/client");
  const root = createRoot(document.getElementById("root")!);
  let entries = [{ written: "■", spoken: "しかく" }];
  invokeImpl.handler = async (command, args) => {
    if (command === "list_tts_dictionary") return entries;
    if (command === "search_tts_dictionary_presets") return { total: 80205, entries: [] };
    if (command === "save_tts_dictionary_entry") {
      entries = [(args as { input: { entry: { written: string; spoken: string } } }).input.entry];
      return entries;
    }
    if (command === "preview_tts_dictionary") return undefined;
    throw new Error(`unexpected command: ${command}`);
  };
  try {
    await act(async () => root.render(<TtsDictionaryPage />));
    await act(async () =>
      document.querySelector<HTMLButtonElement>(".tts-dictionary-list > button")!.click(),
    );
    await act(async () =>
      document.querySelector<HTMLInputElement>(".tts-dictionary-skip input")!.click(),
    );
    expect(
      document.querySelector<HTMLInputElement>(".tts-dictionary-editor label:nth-of-type(2) input")
        ?.disabled,
    ).toBe(true);
    await act(async () =>
      document.querySelector<HTMLButtonElement>(".tts-dictionary-preview button")!.click(),
    );
    expect(invokeCalls.find(({ command }) => command === "preview_tts_dictionary")?.args).toEqual({
      input: {
        text: "■に相談してみましょう。",
        original: "■",
        entry: { written: "■", spoken: "" },
      },
    });
    await act(async () =>
      document.querySelector<HTMLButtonElement>(".tts-dictionary-actions .primary")!.click(),
    );
    expect(entries).toEqual([{ written: "■", spoken: "" }]);
    expect(document.querySelector(".tts-dictionary-list > button")?.textContent).toContain(
      "読み飛ばす",
    );
  } finally {
    await act(async () => root.unmount());
    environment.restore();
    resetTauriCoreMock();
  }
});

test("finds a bundled reading and saves a custom override", async () => {
  const environment = installJsdom();
  resetTauriCoreMock();
  const { TtsDictionaryPage } = await import("../src/features/ttsDictionary/TtsDictionaryPage");
  const { createRoot } = await import("react-dom/client");
  const root = createRoot(document.getElementById("root")!);
  let entries: { written: string; spoken: string }[] = [];
  invokeImpl.handler = async (command, args) => {
    if (command === "list_tts_dictionary") return entries;
    if (command === "search_tts_dictionary_presets") {
      const query = (args as { query: string }).query;
      return { total: 80205, entries: query ? [{ written: "銀行", spoken: "ギンコウ" }] : [] };
    }
    if (command === "save_tts_dictionary_entry") {
      const input = (args as { input: { original: string | null; entry: { written: string; spoken: string } } }).input;
      expect(input.original).toBeNull();
      entries = [input.entry];
      return entries;
    }
    throw new Error(`unexpected command: ${command}`);
  };
  try {
    await act(async () => root.render(<TtsDictionaryPage />));
    const search = document.querySelector<HTMLInputElement>(".tts-dictionary-list-tools input")!;
    await act(async () => {
      Object.getOwnPropertyDescriptor(window.HTMLInputElement.prototype, "value")?.set?.call(search, "銀行");
      search.dispatchEvent(new Event("input", { bubbles: true }));
    });
    await act(async () => new Promise((resolve) => setTimeout(resolve, 200)));
    expect(document.querySelector(".tts-dictionary-preset-heading")?.textContent).toContain("80,205");
    await act(async () => document.querySelector<HTMLButtonElement>(".tts-dictionary-presets button")!.click());
    expect(document.querySelector<HTMLInputElement>(".tts-dictionary-editor label:nth-of-type(2) input")?.value).toBe("ギンコウ");
    expect(document.querySelector(".tts-dictionary-actions")?.textContent).not.toContain("削除");
    await act(async () => document.querySelector<HTMLButtonElement>(".tts-dictionary-actions .primary")!.click());
    expect(entries).toEqual([{ written: "銀行", spoken: "ギンコウ" }]);
  } finally {
    await act(async () => root.unmount());
    environment.restore();
    resetTauriCoreMock();
  }
});

test("shows the first 100 presets on opening and searches the remaining words", async () => {
  const environment = installJsdom();
  resetTauriCoreMock();
  const { TtsDictionaryPage } = await import("../src/features/ttsDictionary/TtsDictionaryPage");
  const { createRoot } = await import("react-dom/client");
  const root = createRoot(document.getElementById("root")!);
  const initial = Array.from({ length: 100 }, (_, index) => ({
    written: `語句${String(index).padStart(3, "0")}`,
    spoken: `ゴク${index}`,
  }));
  invokeImpl.handler = async (command, args) => {
    if (command === "list_tts_dictionary") return [];
    if (command === "search_tts_dictionary_presets") {
      return {
        total: 80205,
        entries: (args as { query: string }).query ? [{ written: "銀行", spoken: "ギンコウ" }] : initial,
      };
    }
    throw new Error(`unexpected command: ${command}`);
  };
  try {
    await act(async () => root.render(<TtsDictionaryPage />));
    await act(async () => new Promise((resolve) => setTimeout(resolve, 20)));
    expect(document.querySelectorAll(".tts-dictionary-presets button")).toHaveLength(100);
    expect(document.querySelector(".tts-dictionary-presets")?.textContent).toContain("残りは検索");
    const search = document.querySelector<HTMLInputElement>(".tts-dictionary-list-tools input")!;
    await act(async () => {
      Object.getOwnPropertyDescriptor(window.HTMLInputElement.prototype, "value")?.set?.call(search, "銀行");
      search.dispatchEvent(new Event("input", { bubbles: true }));
    });
    await act(async () => new Promise((resolve) => setTimeout(resolve, 200)));
    expect(document.querySelectorAll(".tts-dictionary-presets button")).toHaveLength(1);
    expect(document.querySelector(".tts-dictionary-presets")?.textContent).toContain("銀行");
  } finally {
    await act(async () => root.unmount());
    environment.restore();
    resetTauriCoreMock();
  }
});

test("rejects an existing custom word while typing a new entry", async () => {
  const environment = installJsdom();
  resetTauriCoreMock();
  const { TtsDictionaryPage } = await import("../src/features/ttsDictionary/TtsDictionaryPage");
  const { createRoot } = await import("react-dom/client");
  const root = createRoot(document.getElementById("root")!);
  invokeImpl.handler = async (command) => {
    if (command === "list_tts_dictionary") return [{ written: "SAAA", spoken: "サー" }];
    if (command === "search_tts_dictionary_presets") return { total: 80205, entries: [] };
    throw new Error(`unexpected command: ${command}`);
  };
  try {
    await act(async () => root.render(<TtsDictionaryPage />));
    const written = document.querySelector<HTMLInputElement>(".tts-dictionary-editor label:first-of-type input")!;
    await act(async () => {
      Object.getOwnPropertyDescriptor(window.HTMLInputElement.prototype, "value")?.set?.call(written, "SAAA");
      written.dispatchEvent(new Event("input", { bubbles: true }));
    });
    expect(document.querySelector("#tts-dictionary-duplicate")?.textContent).toContain("カスタム登録語");
    expect(document.querySelector<HTMLButtonElement>(".tts-dictionary-actions .primary")?.disabled).toBe(true);
  } finally {
    await act(async () => root.unmount());
    environment.restore();
    resetTauriCoreMock();
  }
});

test("rejects a preset word while typing a new entry", async () => {
  const environment = installJsdom();
  resetTauriCoreMock();
  const { TtsDictionaryPage } = await import("../src/features/ttsDictionary/TtsDictionaryPage");
  const { createRoot } = await import("react-dom/client");
  const root = createRoot(document.getElementById("root")!);
  invokeImpl.handler = async (command) => {
    if (command === "list_tts_dictionary") return [];
    if (command === "search_tts_dictionary_presets") return { total: 80205, entries: [] };
    if (command === "lookup_tts_dictionary_entry") return { source: "preset", entry: { written: "銀行", spoken: "ギンコウ" } };
    throw new Error(`unexpected command: ${command}`);
  };
  try {
    await act(async () => root.render(<TtsDictionaryPage />));
    const written = document.querySelector<HTMLInputElement>(".tts-dictionary-editor label:first-of-type input")!;
    await act(async () => {
      Object.getOwnPropertyDescriptor(window.HTMLInputElement.prototype, "value")?.set?.call(written, "銀行");
      written.dispatchEvent(new Event("input", { bubbles: true }));
    });
    expect(document.querySelector("#tts-dictionary-duplicate")?.textContent).toContain("既定語彙");
    expect(document.querySelector<HTMLButtonElement>(".tts-dictionary-actions .primary")?.disabled).toBe(true);
  } finally {
    await act(async () => root.unmount());
    environment.restore();
    resetTauriCoreMock();
  }
});
