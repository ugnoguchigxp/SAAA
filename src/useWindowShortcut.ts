import { useEffect, useLayoutEffect, useRef } from "react";

/** One listener, with the callback from the most recently committed render. */
export function useWindowShortcut(onShortcut: (event: KeyboardEvent) => void) {
  const callback = useRef(onShortcut);
  useLayoutEffect(() => {
    callback.current = onShortcut;
  }, [onShortcut]);
  useEffect(() => {
    const listener = (event: KeyboardEvent) => {
      if (!event.defaultPrevented) callback.current(event);
    };
    window.addEventListener("keydown", listener);
    return () => window.removeEventListener("keydown", listener);
  }, []);
}
