import { spawn, type ChildProcess } from "node:child_process";
import { mkdtempSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

export type BrowserSession = { text: string; pid: number; profile: string };

export async function browserText(chrome: string, url: string): Promise<BrowserSession> {
  const profile = mkdtempSync(join(tmpdir(), "saaa-lab-chrome-"));
  const child = spawn(
    chrome,
    [
      "--headless=new",
      "--disable-gpu",
      "--no-first-run",
      `--user-data-dir=${profile}`,
      "--remote-debugging-port=0",
      "about:blank",
    ],
    { detached: true, stdio: ["ignore", "ignore", "pipe"] },
  );
  try {
    const port = await debuggingPort(child);
    const created = await fetch(`http://127.0.0.1:${port}/json/new?${encodeURIComponent(url)}`, {
      method: "PUT",
    });
    if (!created.ok) throw new Error(`chrome did not open the smoke page: ${created.status}`);
    const page = (await created.json()) as { webSocketDebuggerUrl?: string };
    if (!page.webSocketDebuggerUrl) throw new Error("chrome did not return a page socket");
    return { text: await pollPage(page.webSocketDebuggerUrl), pid: child.pid ?? 0, profile };
  } catch (cause) {
    const error = cause instanceof Error ? cause : new Error(String(cause));
    Object.assign(error, { pid: child.pid, profile });
    throw error;
  }
}

function debuggingPort(child: ChildProcess): Promise<string> {
  return new Promise((resolve, reject) => {
    let buffer = "";
    const timer = setTimeout(
      () => reject(new Error("chrome did not open a debugging port")),
      15_000,
    );
    child.stderr?.on("data", (chunk: Buffer) => {
      buffer += chunk.toString("utf8");
      const match = buffer.match(/DevTools listening on ws:\/\/127\.0\.0\.1:(\d+)/);
      if (!match?.[1]) return;
      clearTimeout(timer);
      resolve(match[1]);
    });
    child.once("exit", () => {
      clearTimeout(timer);
      reject(new Error("chrome exited before its debugging port was ready"));
    });
  });
}

async function pollPage(socketUrl: string): Promise<string> {
  const socket = new WebSocket(socketUrl);
  await new Promise<void>((resolve, reject) => {
    socket.addEventListener("open", () => resolve());
    socket.addEventListener("error", () => reject(new Error("chrome page socket failed")));
  });
  let id = 0;
  const pending = new Map<number, (value: string) => void>();
  socket.addEventListener("message", (event) => {
    let message: {
      id?: number;
      result?: { result?: { value?: unknown }; exceptionDetails?: unknown };
    };
    try {
      message = JSON.parse(String(event.data)) as typeof message;
    } catch {
      return;
    }
    if (message.id === undefined) return;
    const resolve = pending.get(message.id);
    if (!resolve) return;
    pending.delete(message.id);
    const value = message.result?.result?.value;
    resolve(
      value === undefined && message.result?.exceptionDetails
        ? "evaluate failed"
        : String(value ?? ""),
    );
  });
  const evaluate = () => {
    const next = ++id;
    socket.send(
      JSON.stringify({
        id: next,
        method: "Runtime.evaluate",
        params: {
          expression: "document.querySelector('#result')?.textContent ?? ''",
          returnByValue: true,
        },
      }),
    );
    return new Promise<string>((resolve, reject) => {
      const timer = setTimeout(() => {
        pending.delete(next);
        reject(new Error("chrome did not answer"));
      }, 5_000);
      pending.set(next, (value) => {
        clearTimeout(timer);
        resolve(value);
      });
    });
  };
  try {
    const deadline = Date.now() + 30_000;
    while (Date.now() < deadline) {
      const text = await evaluate();
      if (text && text !== "pending") return text;
      await new Promise((resolve) => setTimeout(resolve, 200));
    }
    return "timed out";
  } finally {
    socket.close();
  }
}
