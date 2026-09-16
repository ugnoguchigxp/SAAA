import { spawn, type ChildProcessWithoutNullStreams } from "node:child_process";
import { StringDecoder } from "node:string_decoder";

export type RpcRecord = Record<string, unknown>;

// PI-00 harness only. Production process ownership belongs to the Rust adapter.
export class PiProbeClient {
  readonly events: RpcRecord[] = [];
  readonly process: ChildProcessWithoutNullStreams;
  readonly exited: Promise<number | null>;
  private failure: Error | undefined;
  private buffer = "";
  private bytes = 0;
  private sequence = 0;
  private ended = false;
  private readonly decoder = new StringDecoder("utf8");
  private readonly listeners = new Set<() => void>();

  constructor(binary: string, args: string[], cwd: string, env: NodeJS.ProcessEnv) {
    this.process = spawn(binary, args, { cwd, env, stdio: "pipe", detached: true });
    this.exited = new Promise((resolve) => {
      this.process.once("close", (code) => {
        this.ended = true;
        this.buffer += this.decoder.end();
        if (this.buffer.trim()) this.fail(new Error("truncated-rpc-record"));
        this.notify();
        resolve(code);
      });
      this.process.once("error", (error) => this.fail(error));
    });
    this.process.stdin.on("error", (error) => this.fail(error));
    this.process.stdout.on("data", (chunk: Buffer) => this.consume(chunk));
    // Drain diagnostics, but never publish provider auth or source material.
    this.process.stderr.on("data", (chunk: Buffer) => {
      this.bytes += chunk.length;
      if (this.bytes > 64 * 1024 * 1024) this.fail(new Error("rpc-output-limit"));
    });
  }

  private notify(): void {
    for (const listener of this.listeners) listener();
  }

  private fail(error: Error): void {
    this.failure ??= error;
    this.notify();
  }

  private consume(chunk: Buffer): void {
    this.bytes += chunk.length;
    if (this.bytes > 64 * 1024 * 1024) return this.fail(new Error("rpc-output-limit"));
    this.buffer += this.decoder.write(chunk);
    let end: number;
    while ((end = this.buffer.indexOf("\n")) !== -1) {
      const line = this.buffer.slice(0, end).replace(/\r$/, "");
      this.buffer = this.buffer.slice(end + 1);
      if (Buffer.byteLength(line) > 8 * 1024 * 1024) return this.fail(new Error("rpc-line-limit"));
      if (!line) continue;
      try {
        const value: unknown = JSON.parse(line);
        if (!value || typeof value !== "object" || Array.isArray(value))
          throw new Error("invalid-rpc-record");
        this.events.push(value as RpcRecord);
      } catch {
        return this.fail(new Error("invalid-rpc-json"));
      }
    }
    if (Buffer.byteLength(this.buffer) > 8 * 1024 * 1024)
      return this.fail(new Error("rpc-line-limit"));
    this.notify();
  }

  waitFor(predicate: (value: RpcRecord) => boolean, after = 0, timeoutMs = 30_000) {
    return new Promise<RpcRecord>((resolve, reject) => {
      const finish = (error?: Error, value?: RpcRecord) => {
        clearTimeout(timer);
        this.listeners.delete(check);
        if (error) reject(error);
        else resolve(value!);
      };
      const check = () => {
        if (this.failure) return finish(this.failure);
        const value = this.events.slice(after).find(predicate);
        if (value) return finish(undefined, value);
        if (this.ended) finish(new Error("rpc-process-exited"));
      };
      const timer = setTimeout(() => finish(new Error("rpc-deadline")), timeoutMs);
      this.listeners.add(check);
      check();
    });
  }

  async command(type: string, fields: RpcRecord = {}): Promise<RpcRecord> {
    const id = `probe-${++this.sequence}`;
    const after = this.events.length;
    const waiting = this.waitFor((event) => event.type === "response" && event.id === id, after);
    this.process.stdin.write(`${JSON.stringify({ ...fields, type, id })}\n`);
    return waiting;
  }

  async close(): Promise<void> {
    this.process.stdin.end();
    let timer: ReturnType<typeof setTimeout> | undefined;
    const exited = await Promise.race([
      this.exited.then(() => true),
      new Promise<false>((resolve) => {
        timer = setTimeout(() => resolve(false), 5_000);
      }),
    ]);
    clearTimeout(timer);
    if (!exited) {
      if (this.process.pid) {
        try {
          process.kill(-this.process.pid, "SIGKILL");
        } catch {
          this.process.kill("SIGKILL");
        }
      }
      await this.exited;
      throw new Error("rpc-cleanup-timeout");
    }
    if (this.failure) throw this.failure;
    if (this.process.exitCode !== 0) throw new Error("rpc-nonzero-exit");
  }
}

export function dataOf(response: RpcRecord): RpcRecord {
  if (response.success !== true) throw new Error(`rpc-rejected:${response.command}`);
  const data = response.data;
  if (!data || typeof data !== "object" || Array.isArray(data)) throw new Error("rpc-data-missing");
  return data as RpcRecord;
}
