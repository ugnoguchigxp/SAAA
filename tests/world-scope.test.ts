import { describe, expect, test } from "bun:test";
import { resolveWorldSelection } from "../src/features/chat/useWorldScope";
import type { WorldContextStatus } from "../src/lib/generated/runtimeEvent";
const status: WorldContextStatus = {
  choices: [
    { key: "user:u", label: "個人", refs: [{ kind: "user", id: "u", relation: "focus" }] },
    { key: "project:a", label: "同名", refs: [{ kind: "project", id: "a", relation: "focus" }] },
    { key: "project:b", label: "同名", refs: [{ kind: "project", id: "b", relation: "focus" }] },
  ],
  messageScopes: {},
  latestScopeKeys: ["project:a"],
  latestProvider: "fixture",
  delivery: "sent",
  omissionReason: null,
};
describe("wr_t20_scope_selection", () => {
  test("same-name projects use registered identities and preserve original report scope", () => {
    expect(resolveWorldSelection(status, "project:b")?.[0].id).toBe("b");
    expect(status.latestScopeKeys).toEqual(["project:a"]);
    expect(resolveWorldSelection(status, "user:u")?.[0].kind).toBe("user");
  });
  test("deleted targets fail instead of silently switching to another project", () => {
    expect(() => resolveWorldSelection(status, "project:deleted")).toThrow();
  });
});
