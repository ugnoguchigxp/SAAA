import { useLayoutEffect, useRef } from "react";

const focusable =
  'button:not([disabled]), [href], input:not([disabled]), select:not([disabled]), textarea:not([disabled]), [tabindex]:not([tabindex="-1"])';

export function useDialogFocus(open: boolean, onClose: () => void) {
  const dialogRef = useRef<HTMLElement>(null);
  const fallbackRef = useRef<HTMLElement>(null);
  const closeRef = useRef(onClose);
  useLayoutEffect(() => {
    closeRef.current = onClose;
  }, [onClose]);
  useLayoutEffect(() => {
    const dialog = dialogRef.current;
    if (!open || !dialog) return;
    const fallback = fallbackRef.current;
    const origin = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    const controls = () =>
      Array.from(dialog.querySelectorAll<HTMLElement>(focusable)).filter(
        (element) => !element.closest("[hidden], [inert]"),
      );
    (controls()[0] ?? dialog).focus();
    const keydown = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        event.preventDefault();
        event.stopImmediatePropagation();
        closeRef.current();
      } else if (event.key === "Tab") {
        const items = controls();
        const index = items.indexOf(document.activeElement as HTMLElement);
        const next = event.shiftKey ? index - 1 : index + 1;
        event.preventDefault();
        (items.length ? items[(next + items.length) % items.length] : dialog).focus();
      }
    };
    const focusin = (event: FocusEvent) => {
      if (!dialog.contains(event.target as Node)) (controls()[0] ?? dialog).focus();
    };
    window.addEventListener("keydown", keydown, true);
    document.addEventListener("focusin", focusin);
    return () => {
      window.removeEventListener("keydown", keydown, true);
      document.removeEventListener("focusin", focusin);
      (origin?.isConnected ? origin : fallback)?.focus();
    };
  }, [open]);
  return { dialogRef, fallbackRef };
}
