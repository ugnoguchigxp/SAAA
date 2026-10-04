import { expect, test } from "bun:test";
import {
  applyButlerConfiguration,
  defaultRoleRouting,
} from "../src/features/settings/settingsRoleRouting";

test("apply removes an unused shipped Qwen receptionist", () => {
  const current = {
    ...defaultRoleRouting,
    actors: [
      ...defaultRoleRouting.actors,
      {
        id: "larm-frontdesk",
        label: "LARM 受付（backchannel）",
        aliases: [],
        transport: "provider" as const,
        providerId: "lan-llm-dynamic",
        model: null,
        location: "local" as const,
        resourceGroup: "larm-backchannel",
        maxInputBytes: 16000,
        larmProvider: "backchannel" as const,
        capabilities: ["social_reply"],
      },
    ],
    roles: { ...defaultRoleRouting.roles, frontend: "larm-frontdesk" },
  };
  expect(applyButlerConfiguration(current, () => true)?.actors.map((actor) => actor.id)).toEqual([
    "larm-reasoner",
  ]);
});
