export type Capability = (typeof services)[number]["id"];
export const services = [
  { id: "asr", label: "ASR", description: "マイクで録音した音声を文字起こしします。", initial: "" },
  {
    id: "tts",
    label: "TTS",
    description: "入力文を音声に変換して再生できます。",
    initial: "こんにちは。音声合成のテストです。",
  },
  {
    id: "llm",
    label: "Ornith1.5",
    description: "主LLMに入力文を送り、応答を表示します。",
    initial: "短く自己紹介してください。",
  },
  {
    id: "laya",
    label: "Laya",
    description:
      "LARMのLayaに入力文を渡し、左・右・ジャンプ・停止の選択結果と確信度を確認します。会話用の接続設定は変更しません。",
    initial: "右に移動してください。",
  },
  {
    id: "laya-speech",
    label: "Laya（発話表現）",
    description: "読み上げる文章から表情と声の調子を選びます。実際の発話と同じ判定を確認できます。",
    initial: "合格おめでとうございます！本当によかったですね！",
  },
  {
    id: "embedding",
    label: "Embedding",
    description: "入力文のベクトル次元と先頭値を確認します。",
    initial: "これは埋め込みのテストです。",
  },
  {
    id: "image",
    label: "画像生成",
    description: "LARMの画像生成APIに1回だけ依頼し、成果物を表示します。",
    initial: "",
  },
  {
    id: "music",
    label: "楽曲生成",
    description: "LARMの楽曲生成APIに1回だけ依頼し、成果物を再生します。",
    initial: "",
  },
] as const;
