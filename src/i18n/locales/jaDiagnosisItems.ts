export const jaDiagnosisItems = {
  sqlite: "データベース",
  settings: { providers: "モデルプロバイダ設定" },
  harness: {
    reachability: "Harness到達性",
    resolve: "Harness解決",
    embedding: "埋め込み",
    llm: "Harness LLM",
    asr: "音声認識",
    "recent-asr": "直近のマイク利用",
    tts: "音声合成",
  },
  memory: { personal_state: "個人状態" },
  tool_selection: { catalog: "ToolChain" },
  world: { status: "ワールドモデル" },
  context_still: { recall: "ContextStill想起", search: "ContextStill検索" },
  diagnosis: { timeout: "診断タイムアウト", interrupted: "診断の中断" },
} as const;
