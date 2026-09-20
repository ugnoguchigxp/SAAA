import { expect, test } from "bun:test";
import { act, useState } from "react";
import { createRoot } from "react-dom/client";
import { installJsdom } from "./jsdomGlobals";
import { useWindowShortcut } from "../src/useWindowShortcut";
import { useDialogFocus } from "../src/components/useDialogFocus";

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
