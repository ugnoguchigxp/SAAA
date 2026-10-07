import { expect, test } from "bun:test";
import { inflateSync } from "node:zlib";
import { readFileSync } from "node:fs";
import { act } from "react";
import { installJsdom } from "./jsdomGlobals";
import { previewMediaApi } from "../src/features/media/FeatureLabPreview";

test("feature lab preview uses a fixed mock and does not import Tauri", async () => {
  for (const file of [
    "src/features/media/mediaApiModel.ts",
    "src/features/media/FeatureLabPreview.tsx",
  ]) {
    expect(readFileSync(file, "utf8").includes("@tauri-apps")).toBe(false);
  }
  const environment = installJsdom();
  const previousCreate = URL.createObjectURL;
  const previousRevoke = URL.revokeObjectURL;
  URL.createObjectURL = () => "blob:preview";
  URL.revokeObjectURL = () => {};
  const { createRoot } = await import("react-dom/client");
  const { FeatureLabPreview } = await import("../src/features/media/FeatureLabPreview");
  const root = createRoot(document.getElementById("root")!);
  await act(async () => root.render(<FeatureLabPreview scene="unknown" />));
  expect(document.body.textContent).toContain("アバターは描画だけの確認です");
  expect(document.body.textContent).toContain("音声と推論の成功は示しません");
  const textarea = document.querySelector("textarea")!;
  await act(async () => {
    Object.getOwnPropertyDescriptor(window.HTMLTextAreaElement.prototype, "value")!.set!.call(
      textarea,
      "青い円",
    );
    textarea.dispatchEvent(new window.Event("input", { bubbles: true }));
    document
      .querySelector("form")
      ?.dispatchEvent(new window.Event("submit", { bubbles: true, cancelable: true }));
  });
  expect(document.body.textContent).toContain("生成の状態を確認できませんでした");
  await act(async () => root.unmount());
  URL.createObjectURL = previousCreate;
  URL.revokeObjectURL = previousRevoke;
  environment.restore();
});

test("a progress preview stays open until the explicit cancel", async () => {
  const api = previewMediaApi("progress");
  const runId = "00000000-0000-4000-8000-000000000099";
  let settled = false;
  const pending = api.generateMedia({ runId, kind: "image", prompt: "青い円" }, () => {});
  void pending.then(() => {
    settled = true;
  });
  await Promise.resolve();
  expect(settled).toBe(false);
  await api.cancelMedia(runId);
  const output = await pending;
  expect(output.error?.kind).toBe("cancelled");
  expect(output.error?.mayHaveGenerated).toBe(false);
});

test("the preview success artifact is a complete PNG", async () => {
  const bytes = new Uint8Array(await previewMediaApi("success").readMediaArtifact("run", 0));
  expect([...bytes.subarray(0, 8)]).toEqual([137, 80, 78, 71, 13, 10, 26, 10]);
  expect([...bytes.subarray(bytes.length - 8)]).toEqual([73, 69, 78, 68, 174, 66, 96, 130]);
  let offset = 8;
  let inflated: Buffer | undefined;
  while (offset + 8 <= bytes.length) {
    const length = new DataView(bytes.buffer, bytes.byteOffset + offset, 4).getUint32(0);
    const type = Buffer.from(bytes.subarray(offset + 4, offset + 8)).toString("ascii");
    const data = bytes.subarray(offset + 8, offset + 8 + length);
    if (type === "IDAT") inflated = inflateSync(Buffer.from(data));
    offset += 12 + length;
  }
  expect(inflated).toEqual(Buffer.from([0, 255, 0, 0]));
});
