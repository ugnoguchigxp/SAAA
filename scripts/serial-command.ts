import { fileURLToPath } from "node:url";
import { acquireVerificationLock } from "./verification-lock";
import { runVerificationProcess } from "./verification-process";

/** Special build commands use the same project lock as verify and preserve live output. */
export async function runSerialCommand(command: string[], cwd: string): Promise<number> {
  const abort = new AbortController();
  let interrupted = 0;
  const interrupt = () => {
    interrupted = 130;
    abort.abort();
  };
  const terminate = () => {
    interrupted = 143;
    abort.abort();
  };
  process.on("SIGINT", interrupt);
  process.on("SIGTERM", terminate);
  let lock: Awaited<ReturnType<typeof acquireVerificationLock>> | undefined;
  try {
    lock = await acquireVerificationLock(cwd, abort.signal);
    const status = await runVerificationProcess(command, cwd, lock, abort.signal, "inherit");
    return interrupted || status;
  } catch (cause) {
    if (interrupted) return interrupted;
    throw cause;
  } finally {
    lock?.release();
    process.off("SIGINT", interrupt);
    process.off("SIGTERM", terminate);
  }
}

if (import.meta.main) {
  const command = process.argv.slice(2);
  if (command[0] === "--") command.shift();
  if (!command.length) {
    console.error("Usage: bun scripts/serial-command.ts -- <command> [arguments]");
    process.exitCode = 64;
  } else {
    try {
      process.exitCode = await runSerialCommand(
        command,
        fileURLToPath(new URL("..", import.meta.url)),
      );
    } catch (cause) {
      console.error(cause instanceof Error ? cause.message : String(cause));
      process.exitCode = 1;
    }
  }
}
