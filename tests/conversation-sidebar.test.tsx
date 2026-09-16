import { describe, expect, test } from "bun:test";
import { renderToStaticMarkup } from "react-dom/server";
import "../src/i18n";
import { ConversationSidebarFooter } from "../src/components/ConversationSidebarFooter";

describe("sidebar connection footers", () => {
  test("renders active and idle conversation status footers", () => {
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
