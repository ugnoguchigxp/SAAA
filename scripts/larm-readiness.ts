export {
  COMPILED_LARM_CONTRACT_COMMIT,
  FROZEN_LARM_CONTRACT_COMMIT,
  REPORT_FILENAMES,
  REPORT_FORMAT,
  RunnerError,
  emptyReport,
  evaluateReport,
  mergeReports,
  parseCliArguments,
  validateHeaderCredential,
  validateNumericLoopbackOrigin,
  validateReport,
  type ReadinessReport,
  type ReportMode,
} from "./larm-readiness/schema.ts";
export {
  ForbiddenDataScanner,
  appChildEnvironment,
  buildEnvironment,
  loadManifest,
  rustChildEnvironment,
  validateReportDirectory,
} from "./larm-readiness/io.ts";
export { validateSoakObservation } from "./larm-readiness/live.ts";
export {
  aggregateReports,
  atomicWriteReport,
  canonicalBundleDigest,
} from "./larm-readiness/bundle.ts";
export { run } from "./larm-readiness/runner.ts";

import { REPORT_FORMAT, RunnerError, parseCliArguments, type ReportMode } from "./larm-readiness/schema.ts";
import { run } from "./larm-readiness/runner.ts";

async function main(): Promise<void> {
  let mode: ReportMode = "preflight";
  try {
    const arguments_ = parseCliArguments(process.argv.slice(2));
    mode = arguments_.command === "report" ? "aggregate" : arguments_.command === "canary" ? "functional" : arguments_.command === "soak" ? (arguments_.duration === "30m" ? "soak-30m" : "soak-2h") : "preflight";
    const outcome = await run(arguments_);
    process.stdout.write(`${JSON.stringify({ format: REPORT_FORMAT, mode: outcome.mode, result: outcome.result })}\n`);
    process.exitCode = outcome.result === "passed" ? 0 : outcome.result === "failed" ? 2 : 3;
  } catch (error) {
    const known = error instanceof RunnerError ? error : new RunnerError(70, "internal", "failed");
    process.stderr.write(`${known.errorCode}\n`);
    process.stdout.write(`${JSON.stringify({ format: REPORT_FORMAT, mode, result: known.result })}\n`);
    process.exitCode = known.exitCode;
  }
}

if (import.meta.main) await main();
