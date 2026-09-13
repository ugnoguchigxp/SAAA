import { expect, test } from "bun:test";
import { existsSync, mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { runDesktopSmoke } from "../scripts/desktop-smoke-process";

test("cleanup kills descendants even when they close inherited output and ignore SIGTERM", async () => {
  const root = mkdtempSync(join(tmpdir(), "saaa-smoke-descendants-"));
  let descendant: number | undefined;
  try {
    const childScript = `process.on('SIGTERM',()=>{}); require('node:fs').writeFileSync('descendant.pid',String(process.pid)); setInterval(()=>{},1000);`;
    const application = `const fs=require('node:fs'), path=require('node:path'), os=require('node:os');
      require('node:child_process').spawn(process.execPath,['-e',${JSON.stringify(childScript)}],{stdio:'ignore'});
      const ready=setInterval(()=>{if(fs.existsSync('descendant.pid')){
        fs.writeFileSync(path.join(os.tmpdir(),'saaa-frontend-'+process.env.SAAA_SMOKE_MARKER_ID+'.ready'),'ready'); clearInterval(ready);
      }},10); setInterval(()=>{},1000);`;
    await runDesktopSmoke({
      root,
      reportDir: join(root, "report"),
      build: [process.execPath, "-e", "process.exit(0)"],
      executable: [process.execPath, "-e", application],
      buildTimeoutMs: 5_000,
      readyTimeoutMs: 5_000,
    });
    descendant = Number(readFileSync(join(root, "descendant.pid"), "utf8"));
    for (let attempt = 0; attempt < 50; attempt++) {
      try {
        process.kill(descendant, 0);
      } catch {
        return;
      }
      await Bun.sleep(20);
    }
    expect(() => process.kill(descendant!, 0)).toThrow();
  } finally {
    if (!descendant && existsSync(join(root, "descendant.pid")))
      descendant = Number(readFileSync(join(root, "descendant.pid"), "utf8"));
    if (descendant) {
      try {
        process.kill(descendant, "SIGKILL");
      } catch {
        /* Already reaped. */
      }
    }
    rmSync(root, { recursive: true, force: true });
  }
}, 20_000);
