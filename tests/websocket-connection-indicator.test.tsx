import { describe, expect, test } from "bun:test";
import { renderToStaticMarkup } from "react-dom/server";
import { WebSocketConnectionIndicator } from "../src/components/WebSocketConnectionIndicator";
import type { WebSocketConnectionState } from "../src/lib/contracts";

describe("WebSocket connection indicator", () => {
  test.each([
    ["connected", "接続済み"],
    ["connecting", "接続中…"],
    ["disconnected", "未接続"],
  ] as const)("renders the %s state with an accessible label", (state, statusLabel) => {
    const html = renderToStaticMarkup(
      <WebSocketConnectionIndicator
        state={state satisfies WebSocketConnectionState}
        label="WebSocket"
        statusLabel={statusLabel}
      />,
    );

    expect(html).toContain(`websocket-indicator ${state}`);
    expect(html).toContain('role="status"');
    expect(html).toContain(`aria-label="WebSocket: ${statusLabel}"`);
    expect(html).toContain(statusLabel);
  });
});
