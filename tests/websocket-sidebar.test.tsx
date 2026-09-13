import { describe, expect, test } from "bun:test";
import { renderToStaticMarkup } from "react-dom/server";
import "../src/i18n";
import {
  ConversationSidebarFooter,
  WebSocketSidebarFooter,
} from "../src/components/WebSocketConnectionIndicator";

describe("sidebar connection footers", () => {
  test("renders WebSocket and conversation status footers", () => {
    expect(
      renderToStaticMarkup(
        <WebSocketSidebarFooter
          state="connected"
          settingsActive
          onOpenSettings={() => undefined}
        />,
      ),
    ).toContain("sidebar-settings");
    expect(
      renderToStaticMarkup(
        <ConversationSidebarFooter
          active
          settingsActive={false}
          onOpenSettings={() => undefined}
        />,
      ),
    ).toContain("conversation-status");
    expect(
      renderToStaticMarkup(
        <ConversationSidebarFooter
          active={false}
          settingsActive={false}
          onOpenSettings={() => undefined}
        />,
      ),
    ).toContain("conversation-status");
  });
});
