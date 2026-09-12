/** Offline contract runner. Uses only synthetic fixtures and loopback servers. */
import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";
const root = fileURLToPath(new URL("..", import.meta.url));
const mode = process.argv[2];
if (mode !== "fixture") {
  console.error("Only fixture is available. Live LARM/audio measurements are not implemented or claimed by this runner.");
  process.exit(1);
}
const commands = [
  ["cargo", "test", "--manifest-path", "crates/larm-session/Cargo.toml"],
  ["cargo", "test", "--manifest-path", "crates/reasoning-contract/Cargo.toml"],
  ["cargo", "test", "--manifest-path", "services/reasoning-mcp/Cargo.toml"],
  ["cargo", "test", "--manifest-path", "src-tauri/Cargo.toml", "reasoning"],
  ["cargo", "test", "--manifest-path", "src-tauri/Cargo.toml", "conversation_controller"],
  ["bun", "test", "tests/reasoning-run.test.ts", "tests/larm-voice-owner.test.ts", "tests/larm-voice-drain.test.ts"],
];
for (const [command, ...args] of commands) {
  const result = spawnSync(command, args, { cwd: root, stdio: "inherit" });
  if (result.status !== 0) process.exit(result.status ?? 1);
}
console.log("Reasoning fixtures passed. No live provider or acoustic latency was measured.");
