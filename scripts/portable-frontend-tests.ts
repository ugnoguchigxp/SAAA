import { readdirSync } from "node:fs";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

const root = fileURLToPath(new URL("..", import.meta.url));
const testsDirectory = join(root, "tests");
const platformSpecific = new Set(["desktop-smoke-descendants.test.ts", "desktop-smoke.test.ts"]);
const tests = readdirSync(testsDirectory)
  .filter((name) => /\.test\.tsx?$/.test(name) && !platformSpecific.has(name))
  .sort()
  .map((name) => join(testsDirectory, name));

if (!tests.length) throw new Error("No portable frontend tests found");

const child = Bun.spawn([process.execPath, "test", ...tests], {
  cwd: root,
  env: process.env,
  stdin: "inherit",
  stdout: "inherit",
  stderr: "inherit",
});

process.exitCode = await child.exited;
