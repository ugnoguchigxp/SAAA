# 初回Context試験の条件

実施日: 2026年10月1日（JST）。計画: [Context安定化と初回実動作確認](../../../docs/plans/context-window-prefix-cache-plan.md)。製品概念の正本は[Space](https://chatgpt.com/space/page_9fc5877949748191b556705128f6a2f5)、確認した版はsequence 22。概念・段階の範囲を変更していない。

## buildと比較対象

着手時HEADは `979cf9780c128aba860df916a007e9cf663d3d37`。作業中に別の変更が `bba9454ac624f3afc20ed2570dcc594cb6d067cc` へ入った。多数の他の未コミット変更がある。今回の関連ファイルの実体digestは [source-manifest.json](source-manifest.json)。全体作業ツリーを今回の変更と称しない。

legacyは変更前の配置（Systemに日時・残り回数・辞書pending）を同じ観測buildで再現する比較mode。リリース済みの旧binaryそのものではない。stableのみ固定指示と動的参照を分離する。接続失効処理、公開後の再生成禁止、根拠検証、host案内文のspeech経路修正は両modeに共通。辞書Toolに関する並行変更も両方に含まれる。

## 隔離条件

- 各modeで新規in-memory DBを初期化。履歴、辞書、Scopeとqueueを同じfixtureから開始。元の会話・設定を書き換えない。
- localhostの固定Provider、search／fetch fixture、実queue worker、HTTP transport、回答保存、speech本文捕捉を通す。主試験20ターンに加え既存のASR、取消、訂正、辞書と異常系を実行。
- Memory OFFと `SAAA_MEMORY_ENABLED=1` のONは別プロセス。ONではWorld envelopeが提供され、source／Scope失効を検証。豊富な実ユーザーWorld graphの品質試験ではない。
- tool上限6。Provider fixture広告65,536 tokens、output予約4,096、safety 1,976。最終送信ではローカルbyte予算と追加安全余白も検査。byteを実token計測とは扱わない。
- samplingは両modeとも現行queue設定を維持。job開始時にmodeを決め、Tool往復中は変えない。

## 実モデルの条件

既存LARM addressとprofileを読み取り、隔離DBのharnessに同じaddressと `saaa-conversation-ornith15` を指定。認証は既存の認証経路。実ユーザーDBの設定・Providerは変更していない。requested modelは `Ornith-1.5`、応答モデル名・接続identityはreceiptを参照。世代・cold／warm状態は制御できずunknown。

合成のメモリ計画相談を20ターン予定、10ターン目に音声確認を追加する訂正。モデルは実接続だが、検索・取得・speech出力本文の捕捉はfixture。live Webの検索品質やスピーカーでの実再生を確認した試験ではない。

legacy→stableの順で実施。両方が途中終了し、均等な標本ではない。性能差・非劣性を判断しない。既定はlegacy。stableは環境変数で限定試用できる。
