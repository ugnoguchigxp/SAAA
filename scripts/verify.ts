import { closeSync, mkdtempSync, openSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { verificationPlan, VERIFY_HELP, type VerificationStep } from "./verify-plan";
import { acquireVerificationLock } from "./verification-lock";
import { runVerificationProcess } from "./verification-process";

export const ROOT = fileURLToPath(new URL("..", import.meta.url));

/** File-backed output avoids pipe deadlocks and subprocess output-size limits. */
export async function runVerification(steps: VerificationStep[], cwd = ROOT): Promise<number> {
  const directory = mkdtempSync(join(tmpdir(), "saaa-verify-"));
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
    for (const [index, step] of steps.entries()) {
      const log = join(directory, `${index}.log`);
      const descriptor = openSync(log, "w");
      let status = 1;
      let error: unknown;
      try {
        status = await runVerificationProcess(step.command, cwd, lock, abort.signal, descriptor);
      } catch (cause) {
        error = cause;
      } finally {
        closeSync(descriptor);
      }
      if (status !== 0 || interrupted || error) {
        console.error(`FAIL ${step.name}\n$ ${step.command.join(" ")}`);
        // Do not summarize or truncate the failed command's stdout/stderr.
        const output = readFileSync(log);
        if (output.length) await Bun.write(Bun.stderr, output);
        if (error) console.error(error);
        const code = interrupted || (status > 0 ? status : 1);
        console.error(`Exit status: ${code}`);
        return code;
      }
    }
    console.log("OK");
    return 0;
  } catch (cause) {
    if (interrupted) return interrupted;
    throw cause;
  } finally {
    lock?.release();
    process.off("SIGINT", interrupt);
    process.off("SIGTERM", terminate);
    rmSync(directory, { recursive: true, force: true });
  }
}

if (import.meta.main) {
  try {
    const args = process.argv.slice(2);
    if (args.length === 1 && (args[0] === "--help" || args[0] === "-h")) {
      console.log(VERIFY_HELP);
    } else {
      process.exitCode = await runVerification(verificationPlan(args, ROOT));
    }
  } catch (cause) {
    console.error(cause instanceof Error ? cause.message : String(cause));
    process.exitCode = 1;
  }
}
