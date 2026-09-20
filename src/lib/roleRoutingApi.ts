import { invoke } from "@tauri-apps/api/core";
import type {
  RoutingEventRecord,
  RoutingLearningSnapshot,
  RoutingSnapshot,
} from "./generated/runtimeEvent";

export function getRoutingSnapshot(conversationId: string): Promise<RoutingSnapshot> {
  return invoke<RoutingSnapshot>("get_routing_snapshot", {
    input: { conversationId },
  });
}

export function replayRoutingEvents(
  rootId: string,
  afterSeq: bigint,
): Promise<RoutingEventRecord[]> {
  return invoke<RoutingEventRecord[]>("replay_routing_events", {
    input: { rootId, afterSeq },
  });
}

export function getRoutingLearningSnapshot(): Promise<RoutingLearningSnapshot> {
  return invoke<RoutingLearningSnapshot>("get_routing_learning_snapshot");
}

export function runRoutingLearningOnce(): Promise<RoutingLearningSnapshot> {
  return invoke<RoutingLearningSnapshot>("run_routing_learning_once");
}
