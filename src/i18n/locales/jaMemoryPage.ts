import { worldReview } from "./jaWorldReview";
export const jaMemoryPage = {
  review: worldReview,
  askLabel: "Worldへの質問",
  askPlaceholder: "例: キャッシュが変わると何が影響を受ける？",
  askSubmit: "会話で質問する",
  correctionPrompt: "訂正する対象と正しい内容を入力してください。現在の会話の範囲で処理します。",

  columns: { content: "内容", kind: "種類", status: "状態", sources: "根拠" },
  disabled: "メモリ抽出は停止中です。",
  enableMaintenance: "会話からの継続構築を有効にする（LocalLLM）",
  workStatus:
    "待機 {{queued}}・実行中 {{running}}・失敗 {{failed}}・保留 {{held}}・未登録 {{deferred}}",
  viewLabel: "表示対象",
  stateView: "会話の状態",
  worldView: "因果ワールドモデル",
  maintenanceStatus: "継続処理: {{reason}}",
  maintenanceReasons: {
    "local-binding-unverified": "LocalLLMの登録済み実行契約を確認できません",
    "not-started": "開始待ち",
    ready: "実行可能",
    "capability-unavailable": "LocalLLMの抽出契約を確認できません",
    "execution-unavailable": "抽出または後処理が中断しました",
    "connection-unavailable": "LocalLLMへの接続待ち",
  },
  contractPending: "メモリ接続の確認が必要です。",
  pending: "未反映 {{count}}件",
  empty: "記録されたメモリはありません。",
  detailLabel: "メモリの詳細",
  correct: "会話で訂正する",
  sources: "根拠",
  noSources: "確認できる根拠はありません。",
  openRecord: "記録を開く",
  forget: "忘れる",
  forgetConfirm:
    "この原文と、そこから作られた状態を忘れます。この操作は元に戻せません。続けますか？",
  select: "項目を選択すると詳細を表示します。",
};
