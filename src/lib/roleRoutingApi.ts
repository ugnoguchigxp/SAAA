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

export function decideRoutingProposal(
  proposalId: string,
  candidateId: string,
  approve: boolean,
): Promise<import("./generated/runtimeEvent").RoutingProposalSnapshot> {
  return invoke("decide_routing_proposal", {
    input: { proposalId, candidateId, approve },
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

export function rollbackAdaptiveArtifact(artifactId: string): Promise<RoutingLearningSnapshot> {
  return invoke<RoutingLearningSnapshot>("rollback_adaptive_artifact", {
    input: { artifactId },
  });
}

export function listAdaptiveEvaluations(): Promise<
  import("./generated/runtimeEvent").EvaluationView[]
> {
  return invoke("list_adaptive_evaluations");
}

export function approveAdaptiveArtifact(
  artifactId: string,
  expectedRevision: bigint | number | null,
): Promise<void> {
  return invoke("approve_adaptive_artifact", {
    input: { artifactId, expectedRevision },
  });
}

export function activateAdaptiveArtifact(
  artifactId: string,
  expectedRevision: number,
): Promise<void> {
  return invoke("activate_adaptive_artifact", {
    input: { artifactId, expectedRevision },
  });
}
