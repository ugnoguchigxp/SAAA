import { expect, test } from "bun:test";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { ROOT } from "../scripts/verify";

test("feature lab preview entry and model stay free of Tauri", () => {
  for (const file of [
    "scripts/feature-lab-preview.tsx",
    "src/features/media/FeatureLabPreview.tsx",
    "src/features/media/mediaApiModel.ts",
  ]) {
    const source = readFileSync(join(ROOT, file), "utf8");
    expect(source).not.toContain("@tauri-apps");
    expect(source).not.toContain("invoke(");
  }
  const preview = readFileSync(join(ROOT, "src/features/media/FeatureLabPreview.tsx"), "utf8");
  expect(preview).toContain("音声と推論の成功は示しません");
  expect(preview).toContain("previewMediaApi");
});
