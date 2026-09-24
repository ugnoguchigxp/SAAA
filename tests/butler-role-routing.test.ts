import { describe, expect, test } from "bun:test";
import { applyButlerConfiguration, defaultRoleRouting } from "../src/features/settings/settingsRoleRouting";

describe("butler role routing", () => {
  test("default matches the LARM butler recipe", () => {
    expect(defaultRoleRouting.enabled).toBe(true);
    expect(defaultRoleRouting.roles.frontend).toBe("larm-frontdesk");
    expect(defaultRoleRouting.roles.reasoner).toBe("larm-reasoner");
    expect(defaultRoleRouting.actors.map((actor) => [actor.id, actor.larmProvider])).toEqual([
      ["larm-frontdesk", "backchannel"],
      ["larm-reasoner", "llm"],
    ]);
    expect(defaultRoleRouting.recipes[0]).toMatchObject({
      id: "00-butler-respond",
      action: "respond",
      roles: ["frontend", "reasoner"],
      enabled: true,
    });
  });

  test("apply keeps other actors and asks before replacing the same id", () => {
    const current = {
      ...defaultRoleRouting,
      enabled: false,
      actors: [
        { ...defaultRoleRouting.actors[0], label: "old desk" },
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
      "larm-frontdesk",
      "other",
      "larm-reasoner",
    ]);
    expect(applied?.actors[0].label).toBe("LARM 受付");
    expect(applied?.recipes.map((recipe) => recipe.id)).toEqual([
      "00-butler-respond",
      "reasoner-response",
    ]);
  });
});
