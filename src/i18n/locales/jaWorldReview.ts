export const worldReview = {
  title: "Worldの振り返り登録",
  off: "停止",
  preview: "候補だけ選定",
  apply: "検証済み候補を自動登録",
  description:
    "確定した会話をLocalLLMで振り返ります。古い知識の現在性を確認できない場合は保留します。",
  inspect: "候補と根拠を確認",
  reasons: {
    queued: "処理待ち",
    selected: "登録候補",
    applied: "登録済み",
    "no-change": "変更なし",
    "already-registered": "既に登録済み",
    "historical-currentness-unverified": "過去の知識・現在性未確認",
    "evidence-window-incomplete": "根拠が上限を超えたため保留",
    "evidence-budget": "条件・根拠が上限を超えたため保留",
    "proposal-input-changed": "提案の根拠が変更",
    "lease-expired": "中断から再開待ち",
  },
};
