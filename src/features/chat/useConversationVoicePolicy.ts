import { type Dispatch, type SetStateAction, useEffect, useRef, useState } from "react";

import { toMessage } from "../../lib/appHelpers";
import type { ConversationVoicePolicySnapshot } from "../../lib/contracts";
import {
  getConversationVoicePolicy,
  resetConversationVoicePolicy,
  updateConversationVoicePolicy,
} from "../../lib/voiceBehaviorRuntime";

export function selectVoicePolicySnapshot(
  current: ConversationVoicePolicySnapshot | null,
  incoming: ConversationVoicePolicySnapshot,
): ConversationVoicePolicySnapshot {
  return current?.conversationId === incoming.conversationId
    && current.policyRevision > incoming.policyRevision
    ? current
    : incoming;
}

export function useConversationVoicePolicy(
  selectedConversationId: string | null,
  setError: Dispatch<SetStateAction<string | null>>,
) {
  const [voicePolicy, setVoicePolicy] = useState<ConversationVoicePolicySnapshot | null>(null);
  const [voicePolicyUpdating, setVoicePolicyUpdating] = useState(false);
  const selectedConversationIdRef = useRef<string | null>(null);
  const requestRef = useRef(0);
  const mutationRef = useRef<{ conversationId: string; token: number } | null>(null);
  const mutationTokenRef = useRef(0);
  const reportedErrorRef = useRef<string | null>(null);
  const setErrorRef = useRef(setError);
  selectedConversationIdRef.current = selectedConversationId;
  setErrorRef.current = setError;

  function acceptVoicePolicy(policy: ConversationVoicePolicySnapshot) {
    if (selectedConversationIdRef.current !== policy.conversationId) return;
    setVoicePolicy((current) => selectVoicePolicySnapshot(current, policy));
  }

  function reportVoicePolicyError(cause: unknown) {
    const message = toMessage(cause);
    reportedErrorRef.current = message;
    setErrorRef.current(message);
  }

  function clearVoicePolicyError() {
    const reported = reportedErrorRef.current;
    if (!reported) return;
    reportedErrorRef.current = null;
    setErrorRef.current((current) => current === reported ? null : current);
  }

  useEffect(() => {
    const request = ++requestRef.current;
    mutationRef.current = null;
    clearVoicePolicyError();
    setVoicePolicyUpdating(false);
    setVoicePolicy(null);
    if (!selectedConversationId) {
      return;
    }
    void loadPolicy(selectedConversationId, request, true);
    return () => {
      requestRef.current += 1;
    };
  }, [selectedConversationId]);

  async function loadPolicy(conversationId: string, request: number, reportError: boolean) {
    try {
      const policy = await getConversationVoicePolicy(conversationId);
      if (request === requestRef.current && selectedConversationIdRef.current === conversationId) {
        acceptVoicePolicy(policy);
      }
    } catch (cause) {
      if (reportError && request === requestRef.current) reportVoicePolicyError(cause);
    }
  }

  async function apply(
    operation: (policy: ConversationVoicePolicySnapshot) => Promise<ConversationVoicePolicySnapshot>,
  ) {
    const policy = voicePolicy;
    if (
      !policy
      || policy.conversationId !== selectedConversationIdRef.current
      || mutationRef.current
    ) return;
    const request = requestRef.current;
    const mutation = {
      conversationId: policy.conversationId,
      token: ++mutationTokenRef.current,
    };
    mutationRef.current = mutation;
    setVoicePolicyUpdating(true);
    try {
      const updated = await operation(policy);
      if (
        request === requestRef.current
        && selectedConversationIdRef.current === policy.conversationId
      ) {
        acceptVoicePolicy(updated);
        clearVoicePolicyError();
      }
    } catch (cause) {
      await loadPolicy(policy.conversationId, request, false);
      if (
        request === requestRef.current
        && selectedConversationIdRef.current === policy.conversationId
      ) reportVoicePolicyError(cause);
    } finally {
      if (mutationRef.current?.token === mutation.token) {
        mutationRef.current = null;
        setVoicePolicyUpdating(false);
      }
    }
  }

  const setConversationSpeechOutput = (speechOutput: "inherit" | "muted") => apply((policy) =>
    updateConversationVoicePolicy({
      conversationId: policy.conversationId,
      speechOutput,
      listeningPace: null,
      expectedRevision: policy.policyRevision,
    }));

  const setConversationListeningPace = (
    listeningPace: "inherit" | "quick" | "balanced" | "patient",
  ) => apply((policy) => updateConversationVoicePolicy({
    conversationId: policy.conversationId,
    speechOutput: null,
    listeningPace,
    expectedRevision: policy.policyRevision,
  }));

  const resetConversationVoiceOverrides = () => apply((policy) =>
    resetConversationVoicePolicy({
      conversationId: policy.conversationId,
      expectedRevision: policy.policyRevision,
    }));

  return {
    voicePolicy: voicePolicy?.conversationId === selectedConversationId ? voicePolicy : null,
    voicePolicyUpdating,
    setVoicePolicy: acceptVoicePolicy,
    clearVoicePolicy: () => setVoicePolicy(null),
    setConversationSpeechOutput,
    setConversationListeningPace,
    resetConversationVoiceOverrides,
  };
}
