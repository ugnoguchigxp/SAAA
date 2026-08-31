import { existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { isAbsolute, join } from "node:path";
import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";

const ROOT = fileURLToPath(new URL("..", import.meta.url));
const FORMAT = "saaa-maximum-performance-v1";

type CommandResult = { name: string; command: string; elapsedMs: number; passed: boolean };
type BenchmarkReport = { format: string; passed: boolean; [key: string]: unknown };

function reportDirectory(): string {
  const index = process.argv.indexOf("--report-dir");
  const path = index >= 0 ? process.argv[index + 1] : undefined;
  if (!path || !isAbsolute(path)) throw new Error("--report-dir must be an absolute path");
  mkdirSync(path, { recursive: true, mode: 0o700 });
  return path;
}

function run(name: string, command: string, args: string[], environment: Record<string, string> = {}): CommandResult {
  const started = performance.now();
  const result = spawnSync(command, args, {
    cwd: ROOT,
    env: { ...process.env, ...environment },
    stdio: "inherit",
  });
  return {
    name,
    command: [command, ...args].join(" "),
    elapsedMs: Math.round(performance.now() - started),
    passed: result.status === 0,
  };
}

function readBenchmark(path: string): BenchmarkReport {
  if (!existsSync(path)) throw new Error(`missing benchmark report: ${path}`);
  const value = JSON.parse(readFileSync(path, "utf8")) as BenchmarkReport;
  if (value.format !== "saaa-performance-gate-v1" || typeof value.passed !== "boolean") {
    throw new Error(`invalid benchmark report: ${path}`);
  }
  return value;
}

function sourceContract(): { passed: boolean; checks: Record<string, boolean> } {
  const larm = readFileSync(join(ROOT, "src-tauri/src/providers/larm/client/mod.rs"), "utf8");
  const openAi = readFileSync(join(ROOT, "src-tauri/src/providers/openai_compatible.rs"), "utf8");
  const stream = readFileSync(join(ROOT, "src-tauri/src/providers/stream/mod.rs"), "utf8");
  const checks = {
    larmLegacyChatIsTestOnly: /#\[cfg\(test\)\]\s*mod chat;/u.test(larm),
    openAiSseProjectionIsTestOnly: /#\[cfg\(test\)\][\s\S]{0,500}fn sse_event_data/u.test(openAi),
    productionStreamUsesWebSocket: stream.includes("llm_websocket"),
    protocolVersionPinned: readFileSync(join(ROOT, "src-tauri/src/providers/llm_websocket/protocol.rs"), "utf8")
      .includes('SUBPROTOCOL: &str = "saaa.llm-stream.v1"'),
  };
  return { passed: Object.values(checks).every(Boolean), checks };
}

function verify(directory: string): void {
  const environment = { SAAA_PERFORMANCE_REPORT_DIR: directory };
  const reuseBenchmarks = process.argv.includes("--reuse-benchmarks");
  const benchmarkCommands = reuseBenchmarks
    ? [
      { name: "streaming-hot-path", command: "validate existing streaming-hot-path.json", elapsedMs: 0, passed: readBenchmark(join(directory, "streaming-hot-path.json")).passed },
      { name: "sqlite-read-path", command: "validate existing sqlite-read-path.json", elapsedMs: 0, passed: readBenchmark(join(directory, "sqlite-read-path.json")).passed },
    ]
    : [
      run("streaming-hot-path", "cargo", ["bench", "--manifest-path", "src-tauri/Cargo.toml", "--bench", "streaming_hot_path"], environment),
      run("sqlite-read-path", "cargo", ["bench", "--manifest-path", "src-tauri/Cargo.toml", "--bench", "sqlite_read_path"], environment),
    ];
  const commands = [
    ...benchmarkCommands,
    run("websocket-contract", "cargo", ["test", "--manifest-path", "src-tauri/Cargo.toml", "--lib", "providers::llm_websocket"]),
    run("event-hub-and-tts", "cargo", ["test", "--manifest-path", "src-tauri/Cargo.toml", "--lib", "runtime::event_hub"]),
    run("streaming-tts", "cargo", ["test", "--manifest-path", "src-tauri/Cargo.toml", "--lib", "voice::streaming_tts"]),
    run("streaming-asr", "cargo", ["test", "--manifest-path", "src-tauri/Cargo.toml", "--lib", "voice::services::streaming_asr"]),
    run("frontend-hot-path", "bun", ["test", "tests/streaming-text-buffer.test.ts", "tests/final-markdown.test.ts", "tests/streaming-performance.test.ts", "tests/meeting-audio-worker.test.ts", "tests/interrupted-streaming-ui.test.ts"]),
  ];
  const source = sourceContract();
  const report = {
    format: FORMAT,
    mode: "controlled-automated",
    generatedAt: new Date().toISOString(),
    profile: "release",
    source,
    commands,
    result: source.passed && commands.every((command) => command.passed) ? "passed" : "failed",
    contentIncluded: false,
  };
  writeFileSync(join(directory, "automated-verification.json"), `${JSON.stringify(report, null, 2)}\n`, { mode: 0o600 });
  if (report.result !== "passed") process.exitCode = 2;
}

function aggregate(directory: string): void {
  const streaming = readBenchmark(join(directory, "streaming-hot-path.json"));
  const sqlite = readBenchmark(join(directory, "sqlite-read-path.json"));
  const automated = JSON.parse(readFileSync(join(directory, "automated-verification.json"), "utf8")) as { result: string };
  const pendingOperatorGates = [
    "G-LLM-03", "G-LLM-04", "G-UI-01", "G-UI-04", "G-UI-05", "G-UI-06",
    "G-TTS-00", "G-TTS-01", "G-TTS-02", "G-TTS-04", "G-ASR-03", "G-AUD-01",
    "G-AUD-02", "G-SOAK-01",
  ];
  const passed = streaming.passed && sqlite.passed && automated.result === "passed";
  const report = {
    format: FORMAT,
    mode: "aggregate",
    generatedAt: new Date().toISOString(),
    automatedResult: passed ? "passed" : "failed",
    releaseResult: pendingOperatorGates.length === 0 && passed ? "passed" : "blocked",
    passedAutomatedGates: ["G-LLM-01", "G-LLM-02", "G-LLM-05", "G-UI-02", "G-UI-03", "G-TTS-03", "G-ASR-01", "G-ASR-02", "G-DB-01", "G-DB-02"],
    pendingOperatorGates,
    contentIncluded: false,
  };
  writeFileSync(join(directory, "aggregate.json"), `${JSON.stringify(report, null, 2)}\n`, { mode: 0o600 });
  process.stdout.write(`${JSON.stringify(report)}\n`);
  if (!passed) process.exitCode = 2;
}

const command = process.argv[2];
try {
  const directory = reportDirectory();
  if (command === "verify") verify(directory);
  else if (command === "report") aggregate(directory);
  else throw new Error("usage: maximum-performance.ts verify|report --report-dir /absolute/path");
} catch (error) {
  process.stderr.write(`${error instanceof Error ? error.message : "performance verification failed"}\n`);
  process.exitCode = 64;
}
