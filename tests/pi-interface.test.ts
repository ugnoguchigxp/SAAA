import { afterEach, expect, test } from "bun:test";
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { PiProbeClient } from "../scripts/pi-interface/rpc-client";

const directories: string[] = [];
afterEach(() => {
  for (const directory of directories.splice(0))
    rmSync(directory, { recursive: true, force: true });
});

function fixture(source: string) {
  const directory = mkdtempSync(join(tmpdir(), "pi-wire-test-"));
  directories.push(directory);
  const script = join(directory, "fixture.js");
  writeFileSync(script, source);
  return new PiProbeClient(process.execPath, [script], directory, {});
}

test("RPC framing retains a split UTF-8 character and embedded Unicode line separator", async () => {
  const client = fixture(`
    const bytes = Buffer.from(JSON.stringify({type:'test',text:'あ\\u2028い'})+'\\n');
    const cut = bytes.indexOf(Buffer.from('あ')) + 1;
    process.stdout.write(bytes.subarray(0, cut));
    setTimeout(() => process.stdout.write(bytes.subarray(cut)), 20);
  `);
  expect((await client.waitFor((event) => event.type === "test")).text).toBe("あ\u2028い");
  await client.close();
});

test("prompt acceptance is separate from settled and asynchronous model failure", async () => {
  const client = fixture(`
    process.stdin.once('data', chunk => {
      const request = JSON.parse(chunk.toString());
      const emit = value => process.stdout.write(JSON.stringify(value)+'\\n');
      emit({type:'response',id:'unrelated',success:true});
      emit({type:'response',id:request.id,success:true});
      setTimeout(() => {
        emit({type:'message_end',message:{stopReason:'error'}});
        emit({type:'agent_settled'});
        process.stdin.destroy();
      }, 20);
    });
  `);
  expect((await client.command("prompt", { message: "test" })).success).toBe(true);
  await client.waitFor((event) => event.type === "agent_settled");
  expect(client.events.some((event) => event.type === "message_end")).toBe(true);
  await client.close();
});

test("truncated output cannot be reported as clean shutdown", async () => {
  const client = fixture(`process.stdout.write('{"type":');`);
  await client.exited;
  await expect(client.close()).rejects.toThrow("truncated-rpc-record");
});

test("a nonzero exit after valid output remains a failure", async () => {
  const client = fixture(
    `process.stdout.write('{"type":"agent_settled"}\\n'); process.exitCode=2;`,
  );
  await client.exited;
  await expect(client.close()).rejects.toThrow("rpc-nonzero-exit");
});
