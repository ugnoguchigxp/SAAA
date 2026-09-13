import { expect, test } from "bun:test";
import { act, useState } from "react";
import { createRoot } from "react-dom/client";
import { installJsdom } from "./jsdomGlobals";
import { useWindowShortcut } from "../src/useWindowShortcut";
import { useDialogFocus } from "../src/components/useDialogFocus";
import { SituationTabs } from "../src/components/SituationTabs";

test("shortcuts use the latest committed callback once and dialogs consume Escape", async () => {
  const env = installJsdom();
  const root = createRoot(document.getElementById("root")!);
  const calls: number[] = [];
  function Harness({ version }: { version: number }) {
    const [open, setOpen] = useState(false);
    useWindowShortcut((event) => {
      if (event.key === "Escape") calls.push(version);
    });
    const { dialogRef, fallbackRef } = useDialogFocus(open, () => setOpen(false));
    return (
      <section ref={fallbackRef} tabIndex={-1}>
        <button id="origin" onClick={() => setOpen(true)}>
          Open
        </button>
        {open && (
          <aside ref={dialogRef} tabIndex={-1} role="dialog">
            <button id="first">First</button>
            <button id="last">Last</button>
          </aside>
        )}
      </section>
    );
  }
  const key = async (key: string, shiftKey = false) =>
    act(() => {
      document.activeElement!.dispatchEvent(
        new env.dom.window.KeyboardEvent("keydown", {
          key,
          shiftKey,
          bubbles: true,
          cancelable: true,
        }),
      );
    });
  try {
    await act(() => root.render(<Harness version={1} />));
    await key("Escape");
    await act(() => root.render(<Harness version={2} />));
    await key("Escape");
    expect(calls).toEqual([1, 2]);
    const origin = document.getElementById("origin")!;
    origin.focus();
    await act(() => origin.click());
    expect(document.activeElement?.id).toBe("first");
    await key("Tab", true);
    expect(document.activeElement?.id).toBe("last");
    await key("Tab");
    expect(document.activeElement?.id).toBe("first");
    await key("Escape");
    expect(document.querySelector('[role="dialog"]')).toBeNull();
    expect(document.activeElement).toBe(origin);
    expect(calls).toEqual([1, 2]);
    await act(() => origin.click());
    origin.remove();
    await key("Escape");
    expect(document.activeElement?.tagName).toBe("SECTION");
    await act(() => root.unmount());
    await key("Escape");
    expect(calls).toEqual([1, 2]);
  } finally {
    env.dom.window.close();
    env.restore();
  }
});

test("Situation tabs support arrow, Home and End with a single tab stop", async () => {
  const env = installJsdom();
  const root = createRoot(document.getElementById("root")!);
  function Harness() {
    const [view, setView] = useState<"overview" | "review">("overview");
    return (
      <SituationTabs
        view={view}
        onChange={setView}
        labels={{ overview: "Overview", review: "Review" }}
      >
        {view}
      </SituationTabs>
    );
  }
  try {
    await act(() => root.render(<Harness />));
    const tabs = Array.from(document.querySelectorAll<HTMLButtonElement>('[role="tab"]'));
    tabs[0].focus();
    for (const [key, index] of [
      ["ArrowRight", 1],
      ["Home", 0],
      ["End", 1],
      ["ArrowLeft", 0],
    ] as const) {
      await act(() =>
        document.activeElement!.dispatchEvent(
          new env.dom.window.KeyboardEvent("keydown", { key, bubbles: true, cancelable: true }),
        ),
      );
      expect(document.activeElement).toBe(tabs[index]);
      expect(tabs[index].getAttribute("aria-selected")).toBe("true");
      expect(tabs.filter((tab) => tab.tabIndex === 0)).toHaveLength(1);
      expect(document.querySelector('[role="tabpanel"]')?.getAttribute("aria-labelledby")).toBe(
        tabs[index].id,
      );
    }
  } finally {
    await act(() => root.unmount());
    env.dom.window.close();
    env.restore();
  }
});
