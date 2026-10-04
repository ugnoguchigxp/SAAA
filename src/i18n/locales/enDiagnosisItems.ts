export const enDiagnosisItems = {
  sqlite: "Database",
  settings: { providers: "Model provider settings" },
  harness: {
    reachability: "Harness reachability",
    resolve: "Harness resolve",
    embedding: "Embedding",
    llm: "Harness LLM",
    asr: "Speech recognition",
    "recent-asr": "Recent microphone use",
    tts: "Speech synthesis",
  },
  memory: { personal_state: "Personal state" },
  tool_selection: { catalog: "ToolChain" },
  world: { status: "World model" },
  context_still: { recall: "ContextStill recall", search: "ContextStill search" },
  diagnosis: { timeout: "Diagnosis timeout", interrupted: "Diagnosis interrupted" },
} as const;
