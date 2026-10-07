import { createHttpMediaApi } from "../src/features/media/mediaHttpApi";

const result = document.querySelector("#result");
if (!(result instanceof HTMLElement)) throw new Error("missing result");

const api = createHttpMediaApi();

function runId(): string {
  return crypto.randomUUID();
}

async function main(): Promise<void> {
  const first = runId();
  const output = await api.generateMedia(
    { runId: first, kind: "image", prompt: "lab fixture" },
    () => {},
  );
  if (!output.result) throw new Error("missing result");
  const bytes = await api.readMediaArtifact(first, 0);
  const bitmap = await createImageBitmap(new Blob([bytes], { type: "image/png" }));
  const decoded = bitmap.width >= 1 && bitmap.height >= 1;
  bitmap.close();
  if (!decoded) throw new Error("image did not decode");
  const history = await api.listMediaGenerations();
  if (!history.some((row) => row.runId === first && row.status === "accepted")) {
    throw new Error("history missed the accepted run");
  }
  await api.cancelMedia(first).catch(() => undefined);
  const afterCancel = await api.listMediaGenerations();
  if (!afterCancel.some((row) => row.runId === first && row.status === "accepted")) {
    throw new Error("cancel rewrote an accepted run");
  }

  const second = runId();
  const controller = new AbortController();
  const pending = fetch("/api/v1/media/runs", {
    method: "POST",
    credentials: "same-origin",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ runId: second, kind: "image", prompt: "lab fixture" }),
    signal: controller.signal,
  });
  let appeared = false;
  for (let attempt = 0; attempt < 30; attempt += 1) {
    const rows = await api.listMediaGenerations();
    if (rows.some((row) => row.runId === second)) {
      appeared = true;
      break;
    }
    await new Promise((resolve) => setTimeout(resolve, 50));
  }
  if (appeared) controller.abort();
  await pending.catch(() => undefined);
  let terminal = "";
  for (let attempt = 0; attempt < 50; attempt += 1) {
    const rows = await api.listMediaGenerations();
    const row = rows.find((item) => item.runId === second);
    if (row && ["accepted", "cancelled", "failed", "unknown"].includes(row.status)) {
      terminal = row.status;
      break;
    }
    await new Promise((resolve) => setTimeout(resolve, 100));
  }
  if (!terminal) throw new Error("reconnect did not observe the second run");
  const replay = await api.listMediaGenerations();
  if (!replay.some((row) => row.runId === second && row.status === terminal)) {
    throw new Error("reconnect changed the stored terminal");
  }
  result.textContent = "ok";
}

main().catch((error: unknown) => {
  result.textContent = error instanceof Error ? `fail ${error.message}` : "fail";
});
