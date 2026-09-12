import { cancelRun } from "./runtime";
import { isReasoningRun, markReasoningCancellation, clearReasoningCancellation } from "./reasoningRun";
export async function cancelReasoningRun(runId: string | null): Promise<void> {
  if (!runId || !isReasoningRun(runId)) return;
  markReasoningCancellation(runId);
  try { await cancelRun(runId); } catch (error) { clearReasoningCancellation(runId); throw error; }
}
