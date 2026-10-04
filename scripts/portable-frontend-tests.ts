import { runFrontendTests } from "./frontend-tests";
process.exitCode = await runFrontendTests(true);
