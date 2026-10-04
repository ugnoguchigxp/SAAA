export type WebviewCommandPlan =
  | { outcome: "reject" }
  | { outcome: "ack-now" }
  | { outcome: "scroll-then-ack" }
  | { outcome: "reduce-then-ack"; action: "select" | "close" | "close-all" };

/** Ack follows the reducer for tab changes, and follows scroll acceptance for scroll. */
export function planWebviewCommand(
  operation: string,
  selected: number,
  tabCount: number,
): WebviewCommandPlan {
  if (operation === "scroll") return { outcome: "scroll-then-ack" };
  if (operation === "close_all_tabs") {
    return tabCount === 0
      ? { outcome: "reject" }
      : { outcome: "reduce-then-ack", action: "close-all" };
  }
  if (tabCount === 0 || selected < 0) return { outcome: "reject" };
  if (operation === "close_tab") return { outcome: "reduce-then-ack", action: "close" };
  if (operation === "next_tab" || operation === "previous_tab") {
    const delta = operation === "next_tab" ? 1 : -1;
    const next = nextWebsiteTabIndex(selected, tabCount, delta);
    return next === selected
      ? { outcome: "ack-now" }
      : { outcome: "reduce-then-ack", action: "select" };
  }
  if (operation === "select_tab") return { outcome: "reduce-then-ack", action: "select" };
  return { outcome: "reject" };
}

export function nextWebsiteTabIndex(selected: number, count: number, delta: number): number | null {
  if (count <= 0) return null;
  return (selected + delta + count) % count;
}
