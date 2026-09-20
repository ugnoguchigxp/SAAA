import { invoke } from "@tauri-apps/api/core";
import type { RoutingRootSnapshot } from "./generated/runtimeEvent";

/** Stops the routing root after its cancellation receipt has been persisted. */
export function cancelRoutingRoot(rootId: string): Promise<RoutingRootSnapshot> {
  return invoke<RoutingRootSnapshot>("cancel_routing_root", {
    input: { rootId },
  });
}
