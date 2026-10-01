export const enMemoryPage = {
  askLabel: "Ask about World knowledge",
  askPlaceholder: "Example: What is affected if caching changes?",
  askSubmit: "Ask in conversation",
  correctionPrompt:
    "Enter the subject and corrected information. This uses the current conversation scope.",

  columns: { content: "Content", kind: "Kind", status: "Status", sources: "Evidence" },
  disabled: "Memory extraction is paused.",
  enableMaintenance: "Enable continuous conversation learning (LocalLLM)",
  workStatus:
    "Queued {{queued}} · Running {{running}} · Failed {{failed}} · Held {{held}} · Deferred {{deferred}}",
  viewLabel: "Memory view",
  stateView: "Conversation state",
  worldView: "Causal world model",
  maintenanceStatus: "Maintenance: {{reason}}",
  maintenanceReasons: {
    "local-binding-unverified":
      "Registered local execution policy is unavailable or does not match",
    "not-started": "Waiting to start",
    ready: "Ready",
    "capability-unavailable": "LocalLLM extraction contract unavailable",
    "execution-unavailable": "Extraction or cleanup interrupted",
    "connection-unavailable": "Waiting for LocalLLM connection",
  },
  contractPending: "The memory connection needs attention.",
  pending: "{{count}} pending",
  empty: "No memory has been recorded.",
  detailLabel: "Memory details",
  correct: "Correct in conversation",
  sources: "Evidence",
  noSources: "No evidence is available.",
  openRecord: "Open record",
  forget: "Forget",
  forgetConfirm: "Forget this source and the state derived from it? This cannot be undone.",
  select: "Select an item to view details.",
};
