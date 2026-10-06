import { readdirSync } from "node:fs";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

const root = fileURLToPath(new URL("..", import.meta.url));
const platformSpecific = new Set(["desktop-smoke-descendants.test.ts", "desktop-smoke.test.ts"]);

/** Bun module mocks persist across files; each contract needs its own module registry. */
export async function runFrontendTests(portable = false, failFast = false): Promise<number> {
  const files = readdirSync(join(root, "tests"))
    .filter((name) => /\.test\.tsx?$/.test(name) && !(portable && platformSpecific.has(name)))
    .sort();
  if (!files.length) throw new Error("No frontend tests found");
  let next = 0,
    failures = 0,
    passedCases = 0,
    completed = 0;
  await Promise.all(
    Array.from({ length: failFast ? 1 : 4 }, async () => {
      while (next < files.length && !(failFast && failures)) {
        const file = files[next++]!;
        const child = Bun.spawn([process.execPath, "test", join(root, "tests", file)], {
          cwd: root,
          env: process.env,
          stdin: "ignore",
          stdout: "pipe",
          stderr: "pipe",
        });
        let expired = false;
        const timer = setTimeout(() => {
          expired = true;
          child.kill();
        }, 120_000);
        const [stdout, stderr, code] = await Promise.all([
          new Response(child.stdout).text(),
          new Response(child.stderr).text(),
          child.exited,
        ]);
        clearTimeout(timer);
        completed++;
        const output = stdout + stderr;
        const cases = Number(output.match(/\n\s*(\d+) pass\n/)?.[1] ?? 0);
        if (code !== 0 || expired) {
          failures++;
          console.error(`FAIL ${file}${expired ? " (deadline exceeded)" : ""}\n${output}`);
        } else {
          passedCases += cases;
          console.log(`PASS ${file} (${cases} cases)`);
        }
      }
    }),
  );
  console.log(
    `Frontend contracts: ${completed - failures}/${completed} files, ${passedCases} passing cases (${files.length - completed} not run).`,
  );
  return failures ? 1 : 0;
}

if (import.meta.main)
  process.exitCode = await runFrontendTests(
    process.argv.includes("--portable"),
    process.argv.includes("--fail-fast"),
  );
