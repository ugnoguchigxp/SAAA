import { verificationPlan, VERIFY_HELP } from "./verify-plan";
import { ROOT, runAffectedCommand, runVerification } from "./verify-run";

export { affectedRunPlan, ROOT, runVerification, writeVerificationReport } from "./verify-run";

if (import.meta.main) {
  try {
    const args = process.argv.slice(2);
    if (args.length === 1 && (args[0] === "--help" || args[0] === "-h")) {
      console.log(VERIFY_HELP);
    } else if (args[0] === "affected") {
      process.exitCode = await runAffectedCommand(args);
    } else {
      process.exitCode = await runVerification(() => verificationPlan(args, ROOT));
    }
  } catch (cause) {
    console.error(cause instanceof Error ? cause.message : String(cause));
    process.exitCode = 1;
  }
}
