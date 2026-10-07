import { expect, test } from "bun:test";

import { createHttpMediaApi } from "../src/features/media/mediaHttpApi";

const runId = "00000000-0000-4000-8000-000000000001";

function jsonResponse(status: number, body: unknown, stream = false): Response {
  if (!stream) return new Response(JSON.stringify(body), { status });
  return new Response(String(body), {
    status,
    headers: { "content-type": "application/x-ndjson" },
  });
}

test("http media api posts once and reads the terminal", async () => {
  const calls: string[] = [];
  const lines = [
    JSON.stringify({
      version: 1,
      runId,
      seq: 1,
      type: "terminal",
      output: {
        runId,
        result: {
          kind: "image",
          model: "lab-image",
          jobId: null,
          artifacts: [
            {
              id: "a",
              contentUrl: "/a",
              metadataUrl: null,
              mimeType: "image/png",
              metadata: {},
            },
          ],
        },
        error: null,
      },
    }),
  ].join("\n");
  globalThis.fetch = (async (input: RequestInfo | URL, init?: RequestInit) => {
    calls.push(`${init?.method ?? "GET"} ${String(input)}`);
    expect(init?.credentials).toBe("same-origin");
    return jsonResponse(200, `${lines}\n`, true);
  }) as typeof fetch;
  const api = createHttpMediaApi();
  const output = await api.generateMedia({ runId, kind: "image", prompt: "円" }, () => {});
  expect(output.result?.model).toBe("lab-image");
  expect(calls.filter((call) => call.startsWith("POST"))).toHaveLength(1);
});

test("http media api keeps status errors and does not post again", async () => {
  const calls: string[] = [];
  globalThis.fetch = (async (input: RequestInfo | URL, init?: RequestInit) => {
    calls.push(`${init?.method ?? "GET"} ${String(input)}`);
    return jsonResponse(409, { error: { code: "duplicate_run", message: "記録済みです。" } });
  }) as typeof fetch;
  const api = createHttpMediaApi();
  await expect(api.generateMedia({ runId, kind: "image", prompt: "円" }, () => {})).rejects.toThrow(
    "記録済みです。",
  );
  expect(calls).toHaveLength(1);
});

test("a dropped stream looks up the saved run and does not post again", async () => {
  const calls: string[] = [];
  globalThis.fetch = (async (input: RequestInfo | URL, init?: RequestInit) => {
    const method = init?.method ?? "GET";
    calls.push(method);
    if (method === "POST") return jsonResponse(200, "", true);
    return jsonResponse(200, [
      {
        runId,
        kind: "image",
        connectionLabel: "Lab LARM",
        model: "lab-image",
        status: "accepted",
        jobId: null,
        result: {
          kind: "image",
          model: "lab-image",
          jobId: null,
          artifacts: [
            { id: "a", contentUrl: "/a", metadataUrl: null, mimeType: "image/png", metadata: {} },
          ],
        },
        error: null,
        updatedAt: "0",
      },
    ]);
  }) as typeof fetch;
  const api = createHttpMediaApi();
  const output = await api.generateMedia({ runId, kind: "image", prompt: "円" }, () => {});
  expect(output.result?.kind).toBe("image");
  expect(calls.filter((call) => call === "POST")).toHaveLength(1);
  expect(calls.filter((call) => call === "GET")).toHaveLength(1);
});

test("artifact bytes stay binary and cancel expects 202", async () => {
  const bytes = new Uint8Array([1, 2, 3]);
  globalThis.fetch = (async (input: RequestInfo | URL, init?: RequestInit) => {
    const url = String(input);
    if (url.endsWith("/cancel")) {
      return jsonResponse(202, { runId, status: "accepted" });
    }
    expect(init?.method ?? "GET").toBe("GET");
    return new Response(bytes, { status: 200 });
  }) as typeof fetch;
  const api = createHttpMediaApi();
  await api.cancelMedia(runId);
  const artifact = await api.readMediaArtifact(runId, 0);
  expect(new Uint8Array(artifact)).toEqual(bytes);
});
