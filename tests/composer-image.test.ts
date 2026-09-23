import { describe, expect, test } from "bun:test";
import { droppedImageFile, imageDragActive } from "../src/features/chat/composerImage";

function files(items: File[]): FileList {
  return Object.assign([...items], {
    item: (index: number) => items[index] ?? null,
  }) as unknown as FileList;
}

describe("composer image drops", () => {
  test("file drags are the only ones treated as images", () => {
    expect(imageDragActive(["text/plain"])).toBe(false);
    expect(imageDragActive(["Files"])).toBe(true);
  });

  test("multiple files are rejected instead of picking one", () => {
    const png = new File([new Uint8Array([1])], "a.png", { type: "image/png" });
    expect(droppedImageFile(files([png, png]))).toBe("multiple");
  });

  test("one file is passed through so the decoder, not the name, decides", () => {
    const namedText = new File([new Uint8Array([1])], "notes.txt", { type: "text/plain" });
    expect(droppedImageFile(files([namedText]))).toBe(namedText);
  });
});
