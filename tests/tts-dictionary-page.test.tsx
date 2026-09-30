import { expect, test } from "bun:test";
import { act } from "react";
import { installJsdom } from "./jsdomGlobals";
import { invokeCalls, invokeImpl, resetTauriCoreMock } from "./tauriCoreMock";

async function setup(initial: { written: string; spoken: string }[]) {
  const environment = installJsdom();
  resetTauriCoreMock();
  const { TtsDictionaryPage } = await import("../src/features/ttsDictionary/TtsDictionaryPage");
  const { createRoot } = await import("react-dom/client");
  const root = createRoot(document.getElementById("root")!);
  let entries = initial;
  invokeImpl.handler = async (command, args) => {
    if (command === "list_tts_dictionary") return entries;
    if (command === "save_tts_dictionary_entry") {
      const input = (
        args as { input: { original: string | null; entry: { written: string; spoken: string } } }
      ).input;
      entries = entries
        .filter((item) => item.written !== input.original && item.written !== input.entry.written)
        .concat(input.entry);
      return entries;
    }
    if (command === "delete_tts_dictionary_entry") {
      entries = entries.filter((item) => item.written !== (args as { written: string }).written);
      return entries;
    }
    if (command === "preview_tts_dictionary") return undefined;
    throw new Error(`unexpected command: ${command}`);
  };
  await act(async () => root.render(<TtsDictionaryPage />));
  return {
    entries: () => entries,
    close: async () => {
      await act(async () => root.unmount());
      environment.restore();
      resetTauriCoreMock();
    },
  };
}

function change(input: HTMLInputElement, value: string) {
  Object.getOwnPropertyDescriptor(window.HTMLInputElement.prototype, "value")?.set?.call(
    input,
    value,
  );
  input.dispatchEvent(new Event("input", { bubbles: true }));
}

test("renders three compact tables without status or presets", async () => {
  const page = await setup([{ written: "今日", spoken: "きょう" }]);
  try {
    expect(document.querySelectorAll(".tts-dictionary-tables table")).toHaveLength(3);
    expect(
      Array.from(document.querySelectorAll(".tts-dictionary-tables th")).map(
        (item) => item.textContent,
      ),
    ).toEqual(["文字", "読み方", "削除", "文字", "読み方", "削除", "文字", "読み方", "削除"]);
    expect(invokeCalls.map((call) => call.command)).toEqual(["list_tts_dictionary"]);
    expect(document.querySelector("textarea")).toBeNull();
  } finally {
    await page.close();
  }
});

test("previews only the focused reading without saving it", async () => {
  const page = await setup([{ written: "今日", spoken: "きょう" }]);
  try {
    const reading = document.querySelector<HTMLInputElement>('[aria-label="今日の読み方"]')!;
    await act(async () => {
      reading.focus();
      change(reading, "キョウ");
    });
    await act(async () =>
      document
        .querySelector<HTMLButtonElement>(".tts-dictionary-toolbar button:last-of-type")!
        .click(),
    );
    expect(invokeCalls.find((call) => call.command === "preview_tts_dictionary")?.args).toEqual({
      input: { text: "キョウ", original: null, entry: null, raw: true },
    });
    expect(page.entries()).toEqual([{ written: "今日", spoken: "きょう" }]);
  } finally {
    await page.close();
  }
});

test("saves a changed cell and deletes its row", async () => {
  const page = await setup([{ written: "SAAA", spoken: "サー" }]);
  try {
    const reading = document.querySelector<HTMLInputElement>('[aria-label="SAAAの読み方"]')!;
    await act(async () => {
      reading.focus();
      change(reading, "サーアー");
      reading.blur();
    });
    expect(page.entries()).toEqual([{ written: "SAAA", spoken: "サーアー" }]);
    await act(async () =>
      document.querySelector<HTMLButtonElement>('[aria-label="SAAAを削除"]')!.click(),
    );
    expect(page.entries()).toEqual([]);
  } finally {
    await page.close();
  }
});

test("adds a row from the table toolbar", async () => {
  const page = await setup([]);
  try {
    await act(async () =>
      document
        .querySelector<HTMLButtonElement>(".tts-dictionary-toolbar button:first-of-type")!
        .click(),
    );
    const written = document.querySelector<HTMLInputElement>('[aria-label="新規の文字"]')!;
    const spoken = document.querySelector<HTMLInputElement>('[aria-label="新規の読み方"]')!;
    await act(async () => {
      change(written, "今日");
      spoken.focus();
    });
    await act(async () => {
      change(spoken, "きょう");
      spoken.blur();
    });
    expect(page.entries()).toEqual([{ written: "今日", spoken: "きょう" }]);
  } finally {
    await page.close();
  }
});

test("shows a bounded number of editable rows for a large dictionary", async () => {
  const entries = Array.from({ length: 120 }, (_, index) => ({
    written: `語${index}`,
    spoken: `よみ${index}`,
  }));
  const page = await setup(entries);
  try {
    expect(document.querySelectorAll(".tts-dictionary-tables tbody tr")).toHaveLength(90);
    await act(async () =>
      document
        .querySelector<HTMLButtonElement>('[aria-label="辞書のページ"] button:last-of-type')!
        .click(),
    );
    expect(document.querySelectorAll(".tts-dictionary-tables tbody tr")).toHaveLength(30);
  } finally {
    await page.close();
  }
});
