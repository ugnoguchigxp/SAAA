export type ToolMode = "none" | "success" | "failure";

export type QualityScenario = {
  id: string;
  category: "direct" | "ambiguous-asr" | "current" | "tool-failure";
  language: "ja" | "en";
  input: string;
  expected: string;
  toolMode: ToolMode;
  toolResult?: string;
};

export const QUALITY_SCENARIOS: QualityScenario[] = [
  {
    id: "direct-01",
    category: "direct",
    language: "ja",
    input: "7×8はいくつですか。",
    expected: "56と端的に答える",
    toolMode: "none",
  },
  {
    id: "ambiguous-asr-01",
    category: "ambiguous-asr",
    language: "ja",
    input: "予算を15か50にして。",
    expected: "金額や単位が重大に曖昧なので一つだけ短く確認する",
    toolMode: "none",
  },
  {
    id: "current-01",
    category: "current",
    language: "ja",
    input: "現在の市場価格を教えて。",
    expected: "検索結果だけを根拠に時点を添えて答える",
    toolMode: "success",
    toolResult:
      "Deterministic search result: requested value is 150.25, updated 2026-08-30 09:00 JST.",
  },
  {
    id: "tool-failure-01",
    category: "tool-failure",
    language: "ja",
    input: "現在の市場価格を教えて。",
    expected: "取得失敗を簡潔に伝え、価格を推測しない",
    toolMode: "failure",
    toolResult: "Tool error: deterministic network failure.",
  },
];
