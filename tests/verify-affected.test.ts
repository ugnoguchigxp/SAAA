import { expect, test } from "bun:test";
import { mkdirSync, mkdtempSync, readFileSync, rmSync, symlinkSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import {
  collectChanges,
  collectNulChanges,
  unquoteGitPath,
} from "../scripts/verify-affected-changes";
import {
  dependentsOf,
  parseManifests,
  previousManifestFiles,
  unionGraphs,
} from "../scripts/verify-affected-graph";
import { mergeOwners, ownershipGaps, SELECTED_ALLOWLIST } from "../scripts/verify-affected-inputs";
import {
  planAffected,
  selectedAllowlistMatches,
  selectedVerificationSteps,
} from "../scripts/verify-affected-plan";
import { affectedRunPlan, ROOT, runVerification, writeVerificationReport } from "../scripts/verify";
import { verificationPlan } from "../scripts/verify-plan";

test("changed paths keep renames, deletes, and untracked files", () => {
  const changes = collectChanges({
    staged: "R100\told name/a.rs\tnew name/a.rs\nD\tremoved.rs\n",
    unstaged: "M\tnew name/a.rs\n",
    untracked: ["notes file.rs"],
    baseKnown: true,
  });
  expect(changes.fallback).toBeNull();
  expect(changes.paths).toContain("old name/a.rs");
  expect(changes.paths).toContain("new name/a.rs");
  expect(changes.paths).toContain("removed.rs");
  expect(changes.paths).toContain("notes file.rs");
  expect(changes.reasons["old name/a.rs"]).toContain("rename-old");
  const quoted = collectChanges({
    staged: 'R100\t"old name/a.rs"\t"new name/a.rs"\n',
    unstaged: "",
    untracked: [],
    baseKnown: true,
  });
  expect(quoted.paths).toContain("old name/a.rs");
  expect(quoted.paths).not.toContain('"old name/a.rs"');
  expect(unquoteGitPath('"\\343\\201\\202"')).toBe("あ");
});

test("an unknown base or git failure widens verification", () => {
  expect(
    collectChanges({ staged: "", unstaged: "", untracked: [], commits: "", baseKnown: false })
      .fallback,
  ).toContain("unknown");
  expect(
    collectChanges({ staged: "", unstaged: "", untracked: ["x"], baseKnown: true, gitFailed: true })
      .fallback,
  ).toContain("git");
});

test("media UI does not select Rust, and registry selects callers", () => {
  const ui = mergeOwners(["src/features/media/MediaGenerationPanel.tsx"]);
  expect(ui.rustPackages).toEqual([]);
  expect(ui.typescriptTests).toContain("tests/media-generation.test.tsx");
  const registry = mergeOwners(["crates/saaa-provider-routing/src/lib.rs"]);
  expect(registry.rustPackages).toContain("src-tauri");
  expect(registry.rustPackages).toContain("crates/saaa-media");
  const avatar = mergeOwners(["src/features/chat/avatar/LightAvatarBackground.tsx"]);
  expect(avatar.rustPackages).toEqual([]);
  expect(mergeOwners(["src-tauri/build.rs"]).fallback).toBe("full");
});

test("reverse dependencies include dev edges and removed packages widen", () => {
  const graph = parseManifests({
    "crates/saaa-media": '[dependencies]\nsaaa-larm-session = { path = "../larm-session" }\n',
    "services/reasoning-mcp":
      '[dependencies]\nsaaa-larm-session = { path = "../../crates/larm-session" }\n',
    "src-tauri": '[dev-dependencies]\nsaaa-media = { path = "../crates/saaa-media" }\n',
  });
  expect(dependentsOf(graph, ["crates/larm-session"]).sort()).toEqual([
    "crates/larm-session",
    "crates/saaa-media",
    "services/reasoning-mcp",
    "src-tauri",
  ]);
  expect(dependentsOf(graph, ["crates/saaa-media"])).toContain("src-tauri");
  expect(parseManifests({ broken: "[" }).fallback).toBeTruthy();
  const removed = unionGraphs(
    parseManifests({ "src-tauri": '[dependencies]\nserde = "1"\n' }),
    parseManifests({
      "src-tauri": '[dependencies]\nsaaa-media = { path = "../crates/saaa-media" }\n',
    }),
  );
  expect(dependentsOf(removed, ["crates/saaa-media"])).toContain("src-tauri");
  const dotted = parseManifests({
    "src-tauri": '[dependencies.saaa-media]\npath = "../crates/saaa-media"\n',
  });
  expect(dotted.fallback).toBeNull();
  expect(dependentsOf(dotted, ["crates/saaa-media"])).toContain("src-tauri");
});

test("normal selection has no build claim and unknown paths fall back", () => {
  const selected = planAffected({
    level: "normal",
    graph: parseManifests({}),
    changes: collectChanges({
      staged: "M\tsrc/features/media/mediaApiModel.ts\n",
      unstaged: "",
      untracked: [],
      baseKnown: true,
    }),
  });
  expect(selected.level).toBe("normal");
  expect(selected.rustPackages).toEqual(["services/feature-lab", "src-tauri"]);
  expect(selected.typescriptTests).toContain("tests/media-http-api.test.ts");
  expect(selected.shadowRunsFullLevel).toBe(true);
  const unknown = planAffected({
    level: "advance",
    graph: parseManifests({}),
    changes: collectChanges({
      staged: "M\tvendor/mystery.bin\n",
      unstaged: "",
      untracked: [],
      baseKnown: true,
    }),
  });
  expect(unknown.fallback).toBeTruthy();
});

test("report file records the step and redacts secrets while stdout stays OK", async () => {
  const directory = mkdtempSync(join(tmpdir(), "saaa-verify-report-"));
  const report = join(directory, "report.json");
  const previous = process.env.SAAA_VERIFY_REPORT;
  process.env.SAAA_VERIFY_REPORT = report;
  try {
    const code = await runVerification(
      [{ name: "secret step", command: [process.execPath, "-e", "console.log('sk-secretvalue')"] }],
      ROOT,
    );
    expect(code).toBe(0);
    const body = readFileSync(report, "utf8");
    expect(body).toContain("secret step");
    expect(body).not.toContain("sk-secretvalue");
    expect(body).toContain("[redacted]");
    expect(body).toContain('"provesInputUnchanged":false');
    expect(body).toContain('"attempts":1');
    expect(body).not.toContain("inputInvariant");
  } finally {
    if (previous === undefined) delete process.env.SAAA_VERIFY_REPORT;
    else process.env.SAAA_VERIFY_REPORT = previous;
    rmSync(directory, { recursive: true, force: true });
  }
});

test("a report directory that cannot be written does not print OK", async () => {
  const previous = process.env.SAAA_VERIFY_REPORT;
  const lines: string[] = [];
  const original = console.log;
  process.env.SAAA_VERIFY_REPORT = join(tmpdir(), "saaa-missing-report-dir", "report.json");
  console.log = (...args: unknown[]) => {
    lines.push(args.map(String).join(" "));
  };
  try {
    await expect(
      runVerification([{ name: "ok step", command: [process.execPath, "-e", ""] }], ROOT),
    ).rejects.toThrow();
    expect(lines.join("\n")).not.toContain("OK");
  } finally {
    console.log = original;
    if (previous === undefined) delete process.env.SAAA_VERIFY_REPORT;
    else process.env.SAAA_VERIFY_REPORT = previous;
  }
});

test("affected normal is the stage-less plan, and the CLI explain entry accepts it", async () => {
  expect(affectedRunPlan("normal", ROOT).map((step) => step.command.join(" "))).toEqual(
    verificationPlan([], ROOT).map((step) => step.command.join(" ")),
  );
  expect(() => verificationPlan(["normal"], ROOT)).toThrow(/Unknown stage/);
  const child = Bun.spawn(
    [process.execPath, join(ROOT, "scripts/verify.ts"), "affected", "--explain"],
    {
      cwd: ROOT,
      stdout: "pipe",
      stderr: "pipe",
    },
  );
  const [stdout, stderr, code] = await Promise.all([
    new Response(child.stdout).text(),
    new Response(child.stderr).text(),
    child.exited,
  ]);
  expect(code).toBe(0);
  expect(stderr).not.toContain("Unknown stage");
  expect(stdout).not.toContain("OK");
  expect(JSON.parse(stdout).level).toBe("normal");
});

test("NUL paths keep spaces, newlines, and both rename endpoints", () => {
  const changes = collectNulChanges({
    staged: Buffer.from("R100\0old name/a.rs\0new\nname/a.rs\0"),
    unstaged: Buffer.from("M\0new\nname/a.rs\0"),
    untracked: Buffer.from("notes file.rs\0"),
    baseKnown: true,
  });
  expect(changes.fallback).toBeNull();
  expect(changes.paths).toContain("old name/a.rs");
  expect(changes.paths).toContain("new\nname/a.rs");
  expect(changes.paths).toContain("notes file.rs");
  expect(
    collectNulChanges({
      staged: Buffer.from("M\0truncated"),
      unstaged: new Uint8Array(),
      untracked: new Uint8Array(),
      baseKnown: true,
    }).fallback,
  ).toContain("git");
});

test("Cargo aliases and target edges stay, and unresolved workspace widens", () => {
  const aliased = parseManifests({
    "src-tauri":
      '[dependencies]\nmedia = { package = "saaa-media", path = "../crates/saaa-media" }\n[target."cfg(unix)".build-dependencies]\nlocal = { path = "../crates/larm-session" }\n',
  });
  expect(aliased.fallback).toBeNull();
  expect(dependentsOf(aliased, ["crates/saaa-media"])).toContain("src-tauri");
  expect(dependentsOf(aliased, ["crates/larm-session"])).toContain("src-tauri");
  expect(
    parseManifests({
      "src-tauri": "[dependencies]\nsaaa-media = { workspace = true }\n",
    }).fallback,
  ).toContain("workspace");
  expect(previousManifestFiles([{ directory: "src-tauri", shown: "error" }]).fallback).toContain(
    "previous manifest",
  );
  expect(
    parseManifests({
      "src-tauri": '[dependencies]\nlocal = { path = "/opt/saaa" }\n',
    }).fallback,
  ).toContain("absolute");
  expect(parseManifests({ "src-tauri": 'target = "nope"\n' }).fallback).toContain("target");
  expect(parseManifests({ "src-tauri": "[dependencies]\nlocal = 1\n" }).fallback).toContain(
    "parsed",
  );
});

test("unregistered frontend input and launcher changes widen", () => {
  expect(mergeOwners(["src/features/chat/unregistered.tsx"]).fallback).toBe("advance");
  expect(mergeOwners(["scripts/feature-lab.ts"]).fallback).toBe("full");
  expect(mergeOwners(["services/feature-lab/src/main.rs"]).fallback).toBe("full");
  const http = mergeOwners(["src/features/media/mediaHttpApi.ts"]);
  expect(http.rustPackages).toEqual([]);
  expect(http.typescriptTests).toContain("tests/media-http-stream.test.ts");
  expect(http.typescriptTests).toContain("tests/media-generation.test.tsx");
  expect(ownershipGaps(http, ["tests/media-http-stream.test.ts"])).toEqual([]);
  expect(ownershipGaps(http, ["tests/missing-contract.test.ts"])).toEqual([
    "tests/missing-contract.test.ts",
  ]);
});

test("selected execution stays on the avatar allowlist and does not shrink full", () => {
  expect(selectedAllowlistMatches(["src/features/chat/avatar/LightAvatarBackground.tsx"])).toBe(
    true,
  );
  expect(
    selectedAllowlistMatches([
      "src/features/chat/avatar/LightAvatarBackground.tsx",
      "package.json",
    ]),
  ).toBe(false);
  expect(selectedAllowlistMatches([])).toBe(false);
  const advance = selectedVerificationSteps("advance", ROOT);
  expect(advance.some((step) => step.command[0] === "cargo")).toBe(false);
  expect(
    advance.some((step) => step.command.includes("vite") && step.command.includes("build")),
  ).toBe(true);
  for (const file of [
    "tests/light-avatar-background.test.tsx",
    "tests/feature-lab-preview.test.ts",
    "tests/feature-lab-preview.test.tsx",
  ]) {
    expect(advance.some((step) => step.command.includes(file))).toBe(true);
  }
  expect(selectedVerificationSteps("normal", ROOT).some((step) => step.command[1] === "test")).toBe(
    false,
  );
  expect(() => selectedVerificationSteps("full", ROOT)).toThrow(/full/);
  expect(SELECTED_ALLOWLIST).not.toContain("src/features/media/mediaHttpApi.ts");
});

test("unknown affected mode and source report paths are rejected", async () => {
  const child = Bun.spawn(
    [process.execPath, join(ROOT, "scripts/verify.ts"), "affected", "--mode", "yolo"],
    { cwd: ROOT, stdout: "pipe", stderr: "pipe" },
  );
  const stderr = await new Response(child.stderr).text();
  expect(await child.exited).not.toBe(0);
  expect(stderr).toContain("Invalid --mode");

  const previous = process.env.SAAA_VERIFY_REPORT;
  const lines: string[] = [];
  const original = console.log;
  process.env.SAAA_VERIFY_REPORT = join(ROOT, "src/verify-report-not-allowed.json");
  console.log = (...args: unknown[]) => {
    lines.push(args.map(String).join(" "));
  };
  try {
    await expect(
      runVerification([{ name: "ok step", command: [process.execPath, "-e", ""] }], ROOT),
    ).rejects.toThrow(/source/);
    expect(lines.join("\n")).not.toContain("OK");
  } finally {
    console.log = original;
    if (previous === undefined) delete process.env.SAAA_VERIFY_REPORT;
    else process.env.SAAA_VERIFY_REPORT = previous;
  }
});

test("report destinations use the closed output rule and cannot follow source symlinks", () => {
  const root = mkdtempSync(join(tmpdir(), "saaa-report-root-"));
  const outside = mkdtempSync(join(tmpdir(), "saaa-report-outside-"));
  const previous = process.env.SAAA_VERIFY_REPORT;
  const save = (path: string) => {
    process.env.SAAA_VERIFY_REPORT = path;
    writeVerificationReport([], 0, 0, undefined, undefined, 1, {}, [], undefined, root);
  };
  try {
    mkdirSync(join(root, "src-tauri/target/verify-reports"), { recursive: true });
    for (const path of ["report.json", "tsconfig.tsbuildinfo", "src-tauri/report.json"]) {
      expect(() => save(join(root, path))).toThrow("outside the repository");
    }
    const allowed = join(root, "src-tauri/target/verify-reports/report.json");
    save(allowed);
    expect(JSON.parse(readFileSync(allowed, "utf8")).status).toBe(0);
    writeFileSync(join(root, "source.txt"), "source contents");
    symlinkSync(join(root, "source.txt"), join(outside, "linked.json"));
    expect(() => save(join(outside, "linked.json"))).toThrow("symlink");
    expect(readFileSync(join(root, "source.txt"), "utf8")).toBe("source contents");
    symlinkSync(root, join(outside, "alias"));
    expect(() => save(join(outside, "alias/report.json"))).toThrow("outside the repository");
  } finally {
    if (previous === undefined) delete process.env.SAAA_VERIFY_REPORT;
    else process.env.SAAA_VERIFY_REPORT = previous;
    rmSync(root, { recursive: true, force: true });
    rmSync(outside, { recursive: true, force: true });
  }
});
