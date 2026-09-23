export const IMAGE_ONLY_PROMPT = "この画像の内容を説明してください";

let armedId: string | null = null;
let claimedListener: (() => void) | null = null;

export function subscribeImageClaimed(listener: () => void) {
  claimedListener = listener;
  return () => {
    if (claimedListener === listener) claimedListener = null;
  };
}

export function notifyImageClaimed() {
  claimedListener?.();
}

export function armTurnImage(id: string | null) {
  armedId = id;
}

export function peekTurnImage() {
  return armedId;
}

export function takeTurnImage() {
  const id = armedId;
  armedId = null;
  return id;
}

export function imageDragActive(types: readonly string[]) {
  return types.includes("Files");
}

export function droppedImageFile(files: FileList | null): File | "multiple" | null {
  if (!files || files.length === 0) return null;
  if (files.length > 1) return "multiple";
  return files[0] ?? null;
}
