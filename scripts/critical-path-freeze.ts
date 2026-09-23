import { createHash } from "node:crypto";
import { readFileSync, readdirSync, statSync, writeFileSync } from "node:fs";
import { join, relative, resolve } from "node:path";

const root = resolve(import.meta.dir, "..");
const manifestPath = join(root, "critical-path-freeze.json");
const domains = {
  asr: [
    "src/features/voice/ambientVoiceCapture.ts",
    "src/features/voice/ambientVoiceCaptureActions.ts",
    "src/features/voice/useAmbientVoiceSession.ts",
    "src/features/voice/voiceCaptureSettings.ts",
    "src/lib/microphone.ts",
    "src/lib/voiceAsrRuntime.ts",
    "src-tauri/src/voice/streaming_asr",
    "src-tauri/src/voice/session/asr.rs",
    "src-tauri/src/voice/session/asr_routes.rs",
    "src-tauri/src/voice/network_asr",
    "src-tauri/src/voice/network_asr.rs",
  ],
  "initial-response": [
    "src/App.tsx",
    "src/lib/ipcValidation.ts",
    "src-tauri/src/persistence/app_commands.rs",
    "src-tauri/src/persistence/schema.rs",
    "src-tauri/src/persistence/settings.d/02.rs",
    "src-tauri/src/role_routing/schema.rs",
    "src-tauri/src/runtime/conversation_turn.rs",
    "src-tauri/src/runtime/conversation_inputs.rs",
    "src-tauri/src/runtime/conversation_prepare.rs",
    "src-tauri/src/runtime/conversation_context.rs",
    "src-tauri/src/runtime/conversation_provider_route.rs",
    "src-tauri/src/runtime/conversation_provider_route.d",
    "src-tauri/src/providers/routing.rs",
    "src-tauri/src/providers/routing",
    "src-tauri/src/providers/openai_compatible.rs",
    "src-tauri/src/providers/dynamic_lan",
    "src-tauri/src/providers/chat_completions",
    "src-tauri/src/providers/agent_session",
    "src-tauri/src/providers/stream/dynamic_lan.rs",
    "src-tauri/src/providers/stream/dynamic_lan",
  ],
} as const;

type Domain = keyof typeof domains;
type Entry = { acceptedAt: string; reason: string; files: Record<string, string> };
type Manifest = { version: 1; domains: Record<Domain, Entry> };

function listFiles(path: string): string[] {
  const absolute = join(root, path);
  if (statSync(absolute).isFile()) return [path];
  return readdirSync(absolute, { withFileTypes: true }).flatMap((entry) => {
    const child = join(path, entry.name);
    return entry.isDirectory()
      ? listFiles(child)
      : entry.isFile() && /\.(rs|ts|tsx)$/.test(child)
        ? [child]
        : [];
  });
}

function snapshot(domain: Domain): Record<string, string> {
  const files = [...new Set(domains[domain].flatMap(listFiles))].sort();
  return Object.fromEntries(
    files.map((file) => [
      file,
      createHash("sha256")
        .update(readFileSync(join(root, file)))
        .digest("hex"),
    ]),
  );
}

function readManifest(): Manifest | null {
  try {
    return JSON.parse(readFileSync(manifestPath, "utf8")) as Manifest;
  } catch (cause) {
    if (cause && typeof cause === "object" && "code" in cause && cause.code === "ENOENT")
      return null;
    throw cause;
  }
}

const [mode = "check", requestedDomain, ...rest] = process.argv.slice(2);
if (mode === "check") {
  const manifest = readManifest();
  if (!manifest || manifest.version !== 1)
    throw new Error("Critical path freeze baseline is missing");
  let changed = false;
  for (const domain of Object.keys(domains) as Domain[]) {
    const expected = manifest.domains[domain]?.files ?? {};
    const current = snapshot(domain);
    const differences = [...new Set([...Object.keys(expected), ...Object.keys(current)])]
      .filter((file) => expected[file] !== current[file])
      .sort();
    if (!differences.length) continue;
    changed = true;
    console.error(
      `${domain} freeze changed:\n${differences.map((file) => `  ${file}`).join("\n")}`,
    );
  }
  if (changed) {
    console.error(
      "Run the relevant regression tests, then accept the intended change with a reason.",
    );
    process.exitCode = 1;
  } else console.log("ASR and initial response implementation freeze verified.");
} else if (mode === "accept") {
  if (requestedDomain !== "asr" && requestedDomain !== "initial-response")
    throw new Error("Choose asr or initial-response");
  if (rest[0] !== "--reason" || !rest[1]?.trim() || rest.length !== 2)
    throw new Error("Provide --reason with a short description of the verified change");
  const manifest = readManifest() ?? { version: 1 as const, domains: {} as Record<Domain, Entry> };
  manifest.domains[requestedDomain] = {
    acceptedAt: new Date().toISOString(),
    reason: rest[1].trim(),
    files: snapshot(requestedDomain),
  };
  writeFileSync(manifestPath, `${JSON.stringify(manifest, null, 2)}\n`);
  console.log(`${requestedDomain} baseline accepted (${relative(root, manifestPath)}).`);
} else throw new Error("Usage: critical-path-freeze.ts check | accept <domain> --reason <text>");
