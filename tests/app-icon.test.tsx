import { describe, expect, test } from "bun:test";
import { renderToStaticMarkup } from "react-dom/server";
import { AppIcon, type AppIconName } from "../src/components/AppIcon";
import { Field, Metric } from "../src/features/settings/SettingsFields";

const names: AppIconName[] = [
  "audit",
  "calendar",
  "chat",
  "mic",
  "model",
  "send",
  "settings",
  "situation",
  "stop",
];

describe("app icons and settings fields", () => {
  test("renders every named icon as an accessible-hidden svg", () => {
    for (const name of names) {
      const html = renderToStaticMarkup(<AppIcon name={name} />);
      expect(html).toContain("aria-hidden");
      expect(html).toContain("<svg");
    }
  });

  test("renders labeled fields and metrics", () => {
    expect(
      renderToStaticMarkup(
        <Field label="Name">
          <input />
        </Field>,
      ),
    ).toContain("Name");
    expect(renderToStaticMarkup(<Metric label="Count" value="3" />)).toContain("3");
  });
});
