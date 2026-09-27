import { describe, expect, test } from "bun:test";
import { applyButlerConfiguration, defaultRoleRouting } from "../src/features/settings/settingsRoleRouting";

describe("butler role routing", () => {
  test("default matches the LARM butler recipe", () => {
    expect(defaultRoleRouting.enabled).toBe(true);
    expect(defaultRoleRouting.roles.frontend).toBeNull();
    expect(defaultRoleRouting.roles.reasoner).toBe("larm-reasoner");
    expect(defaultRoleRouting.actors.map((actor) => [actor.id, actor.larmProvider])).toEqual([
      ["larm-reasoner", "llm"],
    ]);
    expect(defaultRoleRouting.recipes[0]).toMatchObject({
      id: "00-butler-respond",
      action: "respond",
      roles: ["reasoner"],
      enabled: true,
    });
  });

  test("apply keeps other actors and asks before replacing the same id", () => {
    const current = {
      ...defaultRoleRouting,
      enabled: false,
      actors: [
        { ...defaultRoleRouting.actors[0], label: "old reasoner" },
        {
          id: "other",
          label: "Other",
          aliases: [],
          transport: "provider" as const,
          providerId: "other",
          model: null,
          location: "local" as const,
          resourceGroup: "local-llm",
          maxInputBytes: 1024,
          capabilities: ["reason"],
        },
      ],
      recipes: [
        {
          id: "reasoner-response",
          action: "respond" as const,
          roles: ["reasoner"],
          enabled: true,
        },
      ],
    };
    expect(applyButlerConfiguration(current, () => false)).toBeNull();
    const applied = applyButlerConfiguration(current, () => true);
    expect(applied?.enabled).toBe(true);
    expect(applied?.actors.map((actor) => actor.id)).toEqual([
      "larm-reasoner",
      "other",
    ]);
    expect(applied?.actors[0].label).toBe("LARM 思考（llm）");
    expect(applied?.recipes.map((recipe) => recipe.id)).toEqual([
      "00-butler-respond",
      "reasoner-response",
    ]);
  });

  test("apply removes an unused shipped Qwen receptionist", () => {
    const current = {
      ...defaultRoleRouting,
      actors: [...defaultRoleRouting.actors, {
        id: "larm-frontdesk", label: "LARM 受付（backchannel）", aliases: [],
        transport: "provider" as const, providerId: "lan-llm-dynamic", model: null,
        location: "local" as const, resourceGroup: "larm-backchannel",
        maxInputBytes: 16000, larmProvider: "backchannel" as const,
        capabilities: ["social_reply"],
      }],
      roles: { ...defaultRoleRouting.roles, frontend: "larm-frontdesk" },
    };
    expect(applyButlerConfiguration(current, () => true)?.actors.map((actor) => actor.id))
      .toEqual(["larm-reasoner"]);
  });
});
