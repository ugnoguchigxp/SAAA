import { LogicalPosition, LogicalSize } from "@tauri-apps/api/dpi";
import { Webview } from "@tauri-apps/api/webview";

export type PreviewWebviewHandle = {
  label: string;
  setPosition: (x: number, y: number) => Promise<void>;
  setSize: (width: number, height: number) => Promise<void>;
  show: () => Promise<void>;
  hide: () => Promise<void>;
  close: () => Promise<void>;
};

export async function closePreviewWebview(label: string): Promise<void> {
  const webview = await Webview.getByLabel(label);
  if (!webview) return;
  try {
    await webview.hide();
  } catch {
    /* close still runs */
  }
  await webview.close();
}

export async function attachPreviewWebview(label: string): Promise<PreviewWebviewHandle> {
  const webview = await Webview.getByLabel(label);
  if (!webview) throw new Error("preview-create-failed");
  return {
    label,
    setPosition: (x, y) => webview.setPosition(new LogicalPosition(x, y)),
    setSize: (width, height) =>
      webview.setSize(new LogicalSize(Math.max(width, 1), Math.max(height, 1))),
    show: () => webview.show(),
    hide: () => webview.hide(),
    close: () => webview.close(),
  };
}
