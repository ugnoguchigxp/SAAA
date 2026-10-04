import { retainedButlerActors } from "./butlerActors";
import type { RoleRoutingActor, RoleRoutingSettings } from "../../lib/roleRoutingTypes";
import { defaultRoleRoutingSettings } from "./settingsRoleRoutingDefaults";

export const defaultRoleRouting = defaultRoleRoutingSettings();

const butlerActors: RoleRoutingActor[] = defaultRoleRouting.actors;

export function applyButlerConfiguration(
  current: RoleRoutingSettings,
  confirmOverwrite: (actorId: string) => boolean,
): RoleRoutingSettings | null {
  const actors = retainedButlerActors(current);
  for (const actor of butlerActors) {
    const index = actors.findIndex((existing) => existing.id === actor.id);
    if (index >= 0) {
      if (!confirmOverwrite(actor.id)) return null;
      actors[index] = actor;
    } else {
      actors.push(actor);
    }
  }
  const recipes = current.recipes.some((recipe) => recipe.id === "00-butler-respond")
    ? current.recipes.map((recipe) =>
        recipe.id === "00-butler-respond"
          ? { ...recipe, action: "respond" as const, roles: ["reasoner"], enabled: true }
          : recipe,
      )
    : [
        {
          id: "00-butler-respond",
          action: "respond" as const,
          roles: ["reasoner"],
          enabled: true,
        },
        ...current.recipes,
      ];
  return {
    ...current,
    enabled: true,
    actors,
    roles: { ...current.roles, frontend: null, reasoner: "larm-reasoner" },
    recipes,
  };
}
