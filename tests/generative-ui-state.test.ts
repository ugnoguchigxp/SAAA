import { expect, test } from "bun:test";
import { InstanceStateStore } from "../src/features/chat/ui/instanceState";
import type { UiInstance } from "../src/features/chat/ui/api";
const fixture = (id: string) => ({ id, state: {}, stateVersion: 0 }) as UiInstance;
test("state persists serially across unmount and isolates instances of the same view", async () => {
  const writes: { id: string; version: number; value: object }[] = [];
  let release!: () => void;
  const blocked = new Promise<void>((resolve) => {
    release = resolve;
  });
  const store = new InstanceStateStore(async (id, version, value) => {
    writes.push({ id, version, value });
    if (writes.length === 1) await blocked;
    return version + 1;
  });
  store.initialize(fixture("a"));
  store.initialize(fixture("b"));
  const unsubscribe = store.subscribe("a", () => {});
  store.update("a", "filter", "Qwen");
  store.update("a", "filter", "Gemma");
  unsubscribe();
  expect(store.get("b").value).toEqual({});
  release();
  await store.flush("a");
  expect(writes.map((write) => write.version)).toEqual([0, 1]);
  expect(writes[1].value).toEqual({ filter: "Gemma" });
  store.initialize(fixture("a"));
  expect(store.get("a").value.filter).toBe("Gemma");
});
test("failed writes retain dirty state and can retry without silently discarding it", async () => {
  let fail = true;
  const store = new InstanceStateStore(async (_id, version) => {
    if (fail) throw Error("offline");
    return version + 1;
  });
  store.initialize(fixture("a"));
  store.update("a", "page", 2);
  await store.flush("a");
  expect(store.get("a").error).toBe(true);
  expect(store.get("a").dirty).toBe(true);
  fail = false;
  await store.flush("a");
  expect(store.get("a").error).toBe(false);
  expect(store.get("a").version).toBe(1);
});
test("initializing another instance cannot evict it before its first subscription", () => {
  const store = new InstanceStateStore();
  const release: (() => void)[] = [];
  for (let i = 0; i < 150; i++) {
    store.initialize(fixture(String(i)));
    release.push(store.subscribe(String(i), () => {}));
  }
  store.initialize(fixture("new"));
  expect(store.get("new")).toBeDefined();
  release.forEach((fn) => fn());
});
test("state conflict reloads the version and preserves unrelated remote edits", async () => {
  const writes: object[] = [];
  const store = new InstanceStateStore(
    async (_id, version, value) => {
      if (version === 0) throw Error("UI state conflict");
      writes.push(value);
      return version + 1;
    },
    async (id) => ({ ...fixture(id), stateVersion: 3, state: { remoteSort: "status" } }),
  );
  store.initialize(fixture("a"));
  store.update("a", "filter", "Qwen");
  await store.flush("a");
  expect(writes).toEqual([{ remoteSort: "status", filter: "Qwen" }]);
  expect(store.get("a").error).toBe(false);
  expect(store.get("a").version).toBe(4);
});
test("repeated state conflicts stay bounded and retain unsaved changes", async () => {
  let attempts = 0;
  const store = new InstanceStateStore(
    async () => {
      attempts++;
      throw Error("UI state conflict");
    },
    async (id) => fixture(id),
  );
  store.initialize(fixture("a"));
  store.update("a", "filter", "Qwen");
  await store.flush("a");
  expect(attempts).toBe(2);
  expect(store.get("a").error).toBe(true);
  expect(store.get("a").value.filter).toBe("Qwen");
});
test("remount adopts newer persisted state when no local edits are pending", () => {
  const store = new InstanceStateStore();
  store.initialize(fixture("a"));
  store.initialize({ ...fixture("a"), stateVersion: 2, state: { filter: "remote" } });
  expect(store.get("a").version).toBe(2);
  expect(store.get("a").value.filter).toBe("remote");
});
import { UiEnabledStore } from "../src/features/chat/ui/enabledStore";
test("a delayed settings read cannot revert a persisted enable action", async () => {
  let resolve!: (value: boolean) => void;
  const store = new UiEnabledStore(
    () =>
      new Promise((done) => {
        resolve = done;
      }),
    async () => {},
  );
  const loading = store.load();
  await store.set(true);
  resolve(false);
  await loading;
  expect(store.snapshot()).toBe(true);
});
test("failed settings writes do not publish success or poison later writes", async () => {
  let fails = true;
  const store = new UiEnabledStore(
    async () => false,
    async () => {
      if (fails) throw Error("offline");
    },
  );
  await expect(store.set(true)).rejects.toThrow("offline");
  expect(store.snapshot()).toBe(false);
  fails = false;
  await store.set(true);
  expect(store.snapshot()).toBe(true);
});
test("multiple concurrently loading hosts retain state until subscriptions attach", () => {
  const store = new InstanceStateStore();
  const release = Array.from({ length: 150 }, (_, i) => store.retain(fixture(String(i))));
  release.push(store.retain(fixture("next-a")));
  release.push(store.retain(fixture("next-b")));
  expect(store.get("next-a")).toBeDefined();
  expect(store.get("next-b")).toBeDefined();
  release.forEach((fn) => fn());
});
