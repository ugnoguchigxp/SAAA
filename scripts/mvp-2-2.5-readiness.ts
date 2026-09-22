export {
  BUILD_CLASSES,
  MODES,
  REASON_CODES,
  RESULT_VALUES,
  RunnerError,
  SCHEMA_VERSION,
  SUITES,
  aggregateReportSchema,
  caseResultSchema,
  caseSpecs,
  identitySchema,
  observationSchema,
  preflightReportSchema,
  suiteReportSchema,
  type AggregateReport,
  type BuildClass,
  type CaseSpec,
  type CliArguments,
  type Identity,
  type MetricSpec,
  type Mode,
  type PreflightReport,
  type ReasonCode,
  type Result,
  type Suite,
  type SuiteReport,
} from "./mvp-2-2.5-readiness/schema.ts";
export {
  assertMetric,
  classifySigningDetails,
  directoriesOverlap,
  forbiddenDataFindings,
  hashDirectory,
  parseCliArguments,
  writeJsonExclusive,
} from "./mvp-2-2.5-readiness/support.ts";
export {
  aggregateReports,
  hashEvidenceReportSet,
  median,
  runPreflight,
  runVerify,
  summarizeRssMedianDelta,
  validateSuiteCases,
} from "./mvp-2-2.5-readiness/commands.ts";

import { RunnerError } from "./mvp-2-2.5-readiness/schema.ts";
import { aggregateReports, runPreflight, runVerify } from "./mvp-2-2.5-readiness/commands.ts";
import { parseCliArguments } from "./mvp-2-2.5-readiness/support.ts";

async function main() {
  try {
    const args = parseCliArguments(process.argv.slice(2));
    if (args.command === "preflight") {
      runPreflight(args.reportDirectory);
      process.stdout.write("mvp2x preflight: pass\n");
    } else if (args.command === "verify") {
      const report = await runVerify(args.reportDirectory, args.suite!, args.mode!);
      process.stdout.write(`mvp2x ${report.suite}/${report.mode}: ${report.result}\n`);
      if (report.result !== "pass") process.exitCode = 2;
    } else {
      const report = aggregateReports(args.reportDirectory);
      process.stdout.write(`mvp2x aggregate: ${report.result}\n`);
      if (report.result !== "accepted") process.exitCode = 2;
    }
  } catch (cause) {
    if (cause instanceof RunnerError) {
      process.stderr.write(`mvp2x: ${cause.code}\n`);
      process.exitCode = cause.exitCode;
      return;
    }
    process.stderr.write("mvp2x: internal\n");
    process.exitCode = 70;
  }
}

if (import.meta.main) await main();
