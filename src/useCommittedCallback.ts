import { useCallback, useLayoutEffect, useRef } from "react";

/** Stable identity for external subscriptions; only committed renders become visible. */
export function useCommittedCallback<Args extends unknown[], Result>(
  callback: (...args: Args) => Result,
) {
  const current = useRef(callback);
  useLayoutEffect(() => {
    current.current = callback;
  }, [callback]);
  return useCallback((...args: Args) => current.current(...args), []);
}
