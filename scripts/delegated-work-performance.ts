import { spawnSync } from "node:child_process";

const result = spawnSync(
  "cargo",
  [
    "test",
    "--manifest-path",
    "src-tauri/Cargo.toml",
    "--lib",
    "dw_r25_",
    "--",
    "--test-threads=1",
    "--nocapture",
  ],
  { encoding: "utf8" },
);

const output = `${result.stdout ?? ""}${result.stderr ?? ""}`;
const passed = (output.match(/dw_r25_\S+ \.\.\. ok/g) ?? []).length;
console.log(
  JSON.stringify(
    {
      lane: "fixture",
      samples: 32,
      testsPassed: passed,
      driverP95BudgetMs: 2000,
      holdToMessageP95BudgetMs: 2000,
      deadlineBudget: "1 schedule tick + 5s",
      sleep: "not guaranteed; restart reconciliation only",
      liveApp: "not measured this session",
      cargoStatus: result.status,
    },
    null,
    2,
  ),
);
if (result.status !== 0) {
  process.exit(result.status ?? 1);
}
