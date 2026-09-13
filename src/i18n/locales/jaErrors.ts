export const errors = {
  app: {
    conversationActive: "応答を処理中",
    conversationIdle: "会話入力待ち",
    primaryConversationUnavailable:
      "メインの会話を利用できません。SAAAを再起動してからもう一度お試しください。",
    operationFailed: "SAAAはこの操作を完了できませんでした。もう一度お試しください。",
  },
  chat: {
    voiceBlockedDuringMeeting:
      "ミーティングが進行中または一時停止中のため、常時待ち受けを利用できません。",
    voiceSettingsUnavailable: "音声設定を利用できません。",
    recordedAudioUnavailable: "録音した音声を利用できません。もう一度お試しください。",
    voiceQueueFull: "音声処理が混み合っているため、最新の発話は送信しませんでした。",
    voicePendingLimit: "待機中の音声リクエストが多すぎるため、最新の発話は送信しませんでした。",
    speechPlaybackFailed: "読み上げを完了できませんでした。もう一度お試しください。",
    microphoneResumeFailed:
      "常時待ち受けを再開できませんでした。マイクボタンからもう一度お試しください。",
    voiceCaptureInitializationFailed:
      "音声キャプチャを開始できませんでした。マイクボタンからもう一度お試しください。",
    voiceAsrUnavailable:
      "音声認識サービスに接続できません。接続設定を確認してから、マイクボタンでもう一度お試しください。",
    voiceSessionConflict:
      "前回の音声セッションを終了できませんでした。SAAAを再起動してからもう一度お試しください。",
    voiceTargetSpeakerModeUnavailable:
      "本人の声だけを認識する設定は常時待ち受けではまだ利用できません。声の設定で本人確認をオフにしてからお試しください。",
    operationFailed: "会話の操作を完了できませんでした。もう一度お試しください。",
  },
  meeting: {
    transcriptionBackpressure:
      "ミーティングの文字起こし処理が追いついていません。キャプチャを一時停止しましたが、待機中の音声は削除していません。",
    captureInactive: "ミーティングのキャプチャはすでに停止しています。",
    voiceSettingsUnavailable: "音声設定を利用できません。",
    startFailed:
      "ミーティングを開始できませんでした。マイクとASRの設定を確認してからもう一度お試しください。",
    runtimeFailure:
      "ミーティングのランタイムで失敗が発生しました。ミーティング設定を確認してからもう一度お試しください。",
    operationFailed: "ミーティングの操作を完了できませんでした。もう一度お試しください。",
  },
  settings: {
    agentSessionEventStreamMissing:
      "セッション作成には成功しましたが、対応しているイベントストリームの接続先が返されませんでした。Provider側のSSEまたはWebSocket契約を確認してください。",
    operationFailed: "設定を更新できませんでした。内容を確認してからもう一度お試しください。",
  },
  situation: {
    operationFailed: "Situationの操作を完了できませんでした。もう一度お試しください。",
  },
  voice: {
    targetSpeakerRejected:
      "登録した本人の声として確認できなかったため、文字起こしへ送信しませんでした。",
    asrLanguageNotAllowed: "検出された言語は許可されていないため、発話を送信しませんでした。",
    asrLanguageUnknown: "使用言語を判定できなかったため、発話を送信しませんでした。",
    asrNoSpeech: "発話を確認できなかったため、何も送信しませんでした。",
    samplePlaybackFailed: "音声サンプルを再生できませんでした。",
    microphoneStartupTimedOut: "マイクの起動がタイムアウトしました。もう一度お試しください。",
    audioProcessorStartupTimedOut: "音声処理を開始できませんでした。もう一度お試しください。",
    operationFailed: "音声の操作を完了できませんでした。もう一度お試しください。",
  },
  microphone: {
    secureContextRequired:
      "マイクのキャプチャにはSAAAデスクトップアプリまたは安全なローカル接続が必要です。",
    captureUnavailable: "このSAAAビルドではマイクのキャプチャを利用できません。",
    deviceListUnavailable: "このSAAAビルドではマイクデバイスの一覧を利用できません。",
    permissionDenied:
      "マイクへのアクセスが拒否されました。システムのプライバシー設定でSAAAを許可してから、もう一度お試しください。",
    securityBlocked:
      "現在のアプリまたはWebViewのセキュリティポリシーにより、マイクのキャプチャがブロックされています。",
    deviceNotFound:
      "マイクが見つかりません。マイクを接続または有効にしてから、もう一度お試しください。",
    deviceUnavailable:
      "マイクを開けませんでした。他のアプリでの使用を終了するか、デバイスを再接続してからもう一度お試しください。",
    deviceSelectionInvalid:
      "選択したマイクを利用できません。設定で「システム既定」を選んでから、もう一度お試しください。",
    startupInterrupted: "マイクの起動が中断されました。もう一度お試しください。",
    processingCouldNotStart: "マイクの音声処理を開始できませんでした。もう一度お試しください。",
    processingDidNotStart: "マイクの音声処理が開始されませんでした。もう一度お試しください。",
  },
} as const;
