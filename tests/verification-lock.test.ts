import { expect, test } from "bun:test";
import { spawnSync } from "node:child_process";
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { ROOT } from "../scripts/verify";

async function until(check: () => boolean, timeout = 5_000) {
  const deadline = Date.now() + timeout;
  while (!check()) {
    if (Date.now() > deadline) throw new Error("Timed out waiting for fixture progress");
    await Bun.sleep(10);
  }
}

function fixture() {
  const directory = mkdtempSync(join(tmpdir(), "saaa-serial-test-"));
  const git = (...args: string[]) => {
    const result = spawnSync(
      "git",
      ["-c", "user.name=Fixture", "-c", "user.email=fixture@example.invalid", ...args],
      { cwd: directory, encoding: "utf8" },
    );
    if (result.status !== 0) throw new Error(result.stderr);
  };
  git("init", "--quiet");
  git("commit", "--quiet", "--allow-empty", "-m", "fixture");
  const children: ReturnType<typeof Bun.spawn>[] = [];
  const held = (name: string) => `
    const fs = require("node:fs");
    const target = ${JSON.stringify(join(directory, name))};
    if (!fs.existsSync(target)) fs.writeFileSync(target, String(process.pid));
    while (!fs.existsSync(${JSON.stringify(join(directory, `${name}.release`))})) await Bun.sleep(10);
  `;
  const start = (name: string, sources: string[], cwd = directory, env = {}) => {
    const runner = join(directory, `${name}.ts`);
    writeFileSync(
      runner,
      `
      import { runVerification } from ${JSON.stringify(join(ROOT, "scripts/verify.ts"))};
      import { writeFileSync } from "node:fs";
      writeFileSync(${JSON.stringify(join(directory, `${name}.ready`))}, "ready");
      process.exitCode = await runVerification(${JSON.stringify(sources)}.map(source => ({
        name: "fixture", command: [process.execPath, "-e", source]
      })), ${JSON.stringify(cwd)});
    `,
    );
    const child = Bun.spawn([process.execPath, runner], {
      cwd,
      env: { ...process.env, ...env },
      stdout: "pipe",
      stderr: "pipe",
    });
    children.push(child);
    const result = Promise.all([
      child.exited,
      new Response(child.stdout).text(),
      new Response(child.stderr).text(),
    ]).then(([code, stdout, stderr]) => ({ code, stdout, stderr }));
    return { child, result };
  };
  const file = (name: string) => join(directory, name);
  const release = (name: string) => writeFileSync(file(`${name}.release`), "go");
  const cleanup = async () => {
    for (const name of ["first", "second", "third"]) release(name);
    for (const child of children) if (child.exitCode === null) child.kill("SIGTERM");
    await Promise.all(children.map((child) => child.exited));
    rmSync(directory, { recursive: true, force: true });
  };
  return { directory, held, start, file, release, cleanup };
}

test("independent invocations and alternate target directories never overlap", async () => {
  const f = fixture();
  try {
    const first = f.start("a", [f.held("first")]);
    await until(() => existsSync(f.file("first")));
    const second = f.start("b", [f.held("second")], f.directory, {
      CARGO_TARGET_DIR: f.file("other-target"),
    });
    await until(() => existsSync(f.file("b.ready")));
    await Bun.sleep(250);
    expect(existsSync(f.file("second"))).toBe(false);
    f.release("first");
    await until(() => existsSync(f.file("second")));
    f.release("second");
    for (const result of await Promise.all([first.result, second.result])) {
      expect(result).toEqual({ code: 0, stdout: "OK\n", stderr: "" });
    }
  } finally {
    await f.cleanup();
  }
});

test("nested verification inherits the lock without deadlocking", async () => {
  const f = fixture();
  try {
    const nested = f.file("nested.ts");
    writeFileSync(
      nested,
      `
      import { runVerification } from ${JSON.stringify(join(ROOT, "scripts/verify.ts"))};
      process.exitCode = await runVerification([{ name: "nested", command: [process.execPath, "-e",
        ${JSON.stringify(`require("node:fs").writeFileSync(${JSON.stringify(f.file("nested-done"))}, "yes")`)}
      ]}], ${JSON.stringify(f.directory)});
    `,
    );
    const run = f.start("nested-parent", [`await import(${JSON.stringify(nested)})`]);
    await until(() => existsSync(f.file("nested-done")));
    expect(await run.result).toEqual({ code: 0, stdout: "OK\n", stderr: "" });
  } finally {
    await f.cleanup();
  }
});

test("a cancelled waiter starts no command and does not release another owner's lock", async () => {
  const f = fixture();
  try {
    const first = f.start("a", [f.held("first")]);
    await until(() => existsSync(f.file("first")));
    const waiter = f.start("b", [f.held("second")]);
    await until(() => existsSync(f.file("b.ready")));
    waiter.child.kill("SIGTERM");
    expect((await waiter.result).code).toBe(143);
    expect(existsSync(f.file("second"))).toBe(false);
    const next = f.start("c", [f.held("third")]);
    await until(() => existsSync(f.file("c.ready")));
    await Bun.sleep(250);
    expect(existsSync(f.file("third"))).toBe(false);
    f.release("first");
    await until(() => existsSync(f.file("third")));
    f.release("third");
    expect((await first.result).code).toBe(0);
    expect((await next.result).code).toBe(0);
  } finally {
    await f.cleanup();
  }
});

test.skipIf(process.platform === "win32")(
  "SIGKILL recovery waits for the surviving compiler group",
  async () => {
    const f = fixture();
    try {
      const first = f.start("a", [f.held("first")]);
      await until(() => existsSync(f.file("first")));
      first.child.kill("SIGKILL");
      await first.result;
      const second = f.start("b", [f.held("second")]);
      await until(() => existsSync(f.file("b.ready")));
      await Bun.sleep(250);
      expect(existsSync(f.file("second"))).toBe(false);
      f.release("first");
      await until(() => existsSync(f.file("second")));
      f.release("second");
      expect((await second.result).code).toBe(0);
    } finally {
      await f.cleanup();
    }
  },
);

test("failed verification releases the lock for the next invocation", async () => {
  const f = fixture();
  try {
    const failure = f.start("a", ['console.error("fixture failure"); process.exit(7)']);
    expect((await failure.result).code).toBe(7);
    const next = f.start("b", ['console.log("next")']);
    expect(await next.result).toEqual({ code: 0, stdout: "OK\n", stderr: "" });
  } finally {
    await f.cleanup();
  }
});

test("special build commands and verify share a lock", async () => {
  const f = fixture();
  try {
    const first = f.start("a", [f.held("first")]);
    await until(() => existsSync(f.file("first")));
    const runner = f.file("special.ts");
    writeFileSync(
      runner,
      `
      import { runSerialCommand } from ${JSON.stringify(join(ROOT, "scripts/serial-command.ts"))};
      require("node:fs").writeFileSync(${JSON.stringify(f.file("special.ready"))}, "ready");
      process.exitCode = await runSerialCommand([process.execPath, "-e", ${JSON.stringify(f.held("second"))}], ${JSON.stringify(f.directory)});
    `,
    );
    const second = Bun.spawn([process.execPath, runner], { stdout: "ignore", stderr: "ignore" });
    try {
      await until(() => existsSync(f.file("special.ready")));
      await Bun.sleep(250);
      expect(existsSync(f.file("second"))).toBe(false);
      f.release("first");
      await until(() => existsSync(f.file("second")));
      f.release("second");
      expect(await second.exited).toBe(0);
      expect((await first.result).code).toBe(0);
    } finally {
      second.kill("SIGTERM");
      await second.exited;
    }
  } finally {
    await f.cleanup();
  }
});

test("unrelated repositories can still run independently", async () => {
  const a = fixture();
  const b = fixture();
  try {
    const first = a.start("a", [a.held("first")]);
    await until(() => existsSync(a.file("first")));
    const second = b.start("b", [b.held("second")]);
    await until(() => existsSync(b.file("second")));
    a.release("first");
    b.release("second");
    expect((await first.result).code).toBe(0);
    expect((await second.result).code).toBe(0);
  } finally {
    await a.cleanup();
    await b.cleanup();
  }
});

test("cancelling a special command returns the signal status and releases the lock", async () => {
  const f = fixture();
  const runner = f.file("cancel-special.ts");
  writeFileSync(
    runner,
    `import { runSerialCommand } from ${JSON.stringify(join(ROOT, "scripts/serial-command.ts"))};
    process.exitCode = await runSerialCommand([process.execPath, "-e", ${JSON.stringify(f.held("first"))}], ${JSON.stringify(f.directory)});`,
  );
  const child = Bun.spawn([process.execPath, runner], { stdout: "ignore", stderr: "ignore" });
  try {
    await until(() => existsSync(f.file("first")));
    child.kill("SIGTERM");
    expect(await child.exited).toBe(143);
    expect((await f.start("next", ['console.log("next")']).result).code).toBe(0);
  } finally {
    child.kill("SIGTERM");
    await child.exited;
    await f.cleanup();
  }
});

test.skipIf(process.platform === "win32")(
  "cancellation also removes compiler grandchildren that ignore SIGTERM",
  async () => {
    const f = fixture();
    try {
      const descendant = `process.on("SIGTERM", () => {});
      require("node:fs").writeFileSync(${JSON.stringify(f.file("grandchild"))}, String(process.pid));
      await Bun.sleep(60_000);`;
      const first = f.start("a", [
        `Bun.spawn([process.execPath, "-e", ${JSON.stringify(descendant)}], { stdout: "ignore", stderr: "ignore" }); ${f.held("first")}`,
      ]);
      await until(() => existsSync(f.file("grandchild")));
      const pid = Number(readFileSync(f.file("grandchild"), "utf8"));
      first.child.kill("SIGTERM");
      expect((await first.result).code).toBe(143);
      const ps = spawnSync("ps", ["-p", String(pid), "-o", "stat="], { encoding: "utf8" });
      expect(ps.status !== 0 || ps.stdout.trim().startsWith("Z")).toBe(true);
      const next = f.start("b", ['console.log("next")']);
      expect((await next.result).code).toBe(0);
    } finally {
      await f.cleanup();
    }
  },
);

test("Git worktrees and package subdirectories share the same lock", async () => {
  const f = fixture();
  const git = (...args: string[]) => {
    const result = spawnSync("git", args, { cwd: f.directory, encoding: "utf8" });
    if (result.status !== 0) throw new Error(result.stderr);
  };
  try {
    git("init", "--quiet");
    git(
      "-c",
      "user.name=Fixture",
      "-c",
      "user.email=fixture@example.invalid",
      "commit",
      "--quiet",
      "--allow-empty",
      "-m",
      "fixture",
    );
    const worktree = f.file("worktree");
    git("worktree", "add", "--quiet", "--detach", worktree);
    const packageDir = f.file("package");
    mkdirSync(packageDir);
    const first = f.start("a", [f.held("first")], packageDir);
    await until(() => existsSync(f.file("first")));
    const second = f.start("b", [f.held("second")], worktree);
    await until(() => existsSync(f.file("b.ready")));
    await Bun.sleep(250);
    expect(existsSync(f.file("second"))).toBe(false);
    f.release("first");
    await until(() => existsSync(f.file("second")));
    f.release("second");
    expect((await first.result).code).toBe(0);
    expect((await second.result).code).toBe(0);
  } finally {
    await f.cleanup();
  }
});

test.skipIf(process.platform === "win32")(
  "cancellation kills a compiler before another verification starts",
  async () => {
    const f = fixture();
    try {
      const first = f.start("a", [f.held("first")]);
      await until(() => existsSync(f.file("first")));
      const pid = Number(readFileSync(f.file("first"), "utf8"));
      first.child.kill("SIGTERM");
      expect((await first.result).code).toBe(143);
      expect(() => process.kill(pid, 0)).toThrow();
      const next = f.start("b", ['console.log("next")']);
      expect((await next.result).code).toBe(0);
    } finally {
      await f.cleanup();
    }
  },
);

test("cancellation gives a lab-smoke owner time to finish detached-resource cleanup", async () => {
  const f = fixture();
  const smoke = f.file("feature-lab-smoke.ts");
  const ready = f.file("smoke-ready");
  const cleaned = f.file("smoke-cleaned");
  const runner = f.file("smoke-runner.ts");
  writeFileSync(
    smoke,
    `
import { writeFileSync } from "node:fs";
process.on("SIGTERM", async () => {
  await Bun.sleep(2_500);
  writeFileSync(${JSON.stringify(cleaned)}, "cleaned");
  process.exit(143);
});
writeFileSync(${JSON.stringify(ready)}, "ready");
await Bun.sleep(60_000);
`,
  );
  writeFileSync(
    runner,
    `
import { runSerialCommand } from ${JSON.stringify(join(ROOT, "scripts/serial-command.ts"))};
process.exitCode = await runSerialCommand([process.execPath, ${JSON.stringify(smoke)}], ${JSON.stringify(f.directory)});
`,
  );
  const child = Bun.spawn([process.execPath, runner], { stdout: "pipe", stderr: "pipe" });
  try {
    await until(() => existsSync(ready));
    child.kill("SIGTERM");
    expect(await child.exited).toBe(143);
    expect(existsSync(cleaned)).toBe(true);
    expect(await new Response(child.stdout).text()).not.toContain("OK");
    expect(await new Response(child.stderr).text()).toBe("");
    expect((await f.start("after-smoke", ["process.exit(0)"]).result).code).toBe(0);
  } finally {
    child.kill("SIGTERM");
    await child.exited;
    await f.cleanup();
  }
});
