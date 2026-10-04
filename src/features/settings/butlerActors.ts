import type { RoleRoutingSettings, RoleRoutingActor } from "../../lib/roleRoutingTypes";

export function retainedButlerActors(current: RoleRoutingSettings): RoleRoutingActor[] {
  const oldFrontendIsUnused =
    !Object.entries(current.roles).some(
      ([role, id]) => role !== "frontend" && id === "larm-frontdesk",
    ) &&
    !current.recipes.some(
      (recipe) => recipe.id !== "00-butler-respond" && recipe.roles.includes("frontend"),
    );
  return current.actors.filter(
    (actor) =>
      !(
        oldFrontendIsUnused &&
        actor.id === "larm-frontdesk" &&
        actor.providerId === "lan-llm-dynamic" &&
        actor.larmProvider === "backchannel" &&
        actor.resourceGroup === "larm-backchannel"
      ),
  );
}
