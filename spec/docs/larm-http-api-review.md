# LARM通信・設定レビューとWebSocket削除

2026-09-16。対象はSAAAの作業ツリー。既存の未コミット変更を含む現在のコードをレビューした。LARMサーバーの実装・稼働状態は今回検証していない。以下の指摘はWebSocket削除直後の記録。後続の実装内容と残る境界は「SAAA側の後続実装」に記載する。

WebSocketの実装、専用状態管理、UI、依存ライブラリ、旧allocation Provider、専用検証スクリプトを削除した。今後の方針はLLM・ASR・TTSそれぞれをHTTP APIの境界で交換できること。そのためには通信形式に加え、設定の優先順位とフォールバック方針を揃える必要がある。

## レビュー指摘

### 1. P1：LARMモードではASR・TTSの設定変更が実際の接続先に反映されない

`SAAA_CONVERSATION_REASONING_MODE=larm` のとき、ASRは設定から選んだProviderをLARMセッションで上書きする。TTSも音声会話では選択済みProvider・タイムアウトを使わず、LARM・15秒へ固定する。接続先は設定画面のHarness addressとは別に、環境変数 `SAAA_LARM_CONTROL_URL` または `http://192.168.0.130:9810` から取得される。モードはOnceLockに記憶される。

この状態で設定画面からクラウドASRや別TTSに変更しても、LARMを使い続ける。接続障害を設定変更で回避できず、画面上の設定と実効経路が一致しない。

根拠：LARMのモード・接続先（`/Users/y.noguchi/Code/SAAA/src-tauri/src/providers/larm_voice/mod.rs:25`）、ASRの上書き（`/Users/y.noguchi/Code/SAAA/src-tauri/src/voice/streaming_asr/route.rs:50`）、TTSの上書き（`/Users/y.noguchi/Code/SAAA/src-tauri/src/voice/streaming_tts/runtime.rs:94`）。

改善：SAAAで一つの設定解決処理を設け、機能ごとに実効Providerを確定する。環境変数による上書きを残す場合も、画面に実効値と上書き理由を表示する。設定変更時は次の発話・要求から適用し、古いセッションを安全に解放する。受け入れ条件は「LARMのLLMを維持しながらASRだけクラウドへ切り替える」こと。

### 2. P1：四つのProviderが必須で、単一機能の障害を切り離せない

LARMセッションの既定は selector `SAAA` で、`llm`、`backchannel`、`asr`、`tts`、`embedding` をclaimする。`SAAA-w-Image` と `SAAA-w-music` は保存値で指定したときだけ使い、旧 Profile ID は `SAAA` として読む。allowFallbackはfalse、TTLは600秒に固定される。Chat Completions Providerはclaimの`contextWindow`、Embedding Providerは`embeddingSpace`とcapacityを検証する。個別用途では、claimに含まれた単一Providerだけでも有効な契約として扱える。

根拠：固定Provider集合と必須検証（`/Users/y.noguchi/Code/SAAA/crates/larm-session/src/contract.rs:6`）、固定profileと作成条件（`/Users/y.noguchi/Code/SAAA/crates/larm-session/src/lib.rs:104`）。

改善：SAAAでprofileを設定・広告から選択し、実際に使う機能だけを必須にする。LARMには機能単位のready状態・対応API・モデル・制約の広告を依頼する。allowFallback=false自体は不具合ではないが、LARM内の代替選択とSAAAのクラウド切り替えの責任を明示する必要がある。受け入れ条件は「ASR停止中でも、クラウドASR＋LARM LLM＋System TTSで会話できる」こと。

### 3. P2：設定した待ち時間に初期化・解放が含まれず、切り替えが遅れる

Dynamic LANはProvider解決を先に実行し、その後に設定timeout分の推論を開始する。ready待ちは別の300秒期限である。Agent Sessionも作成、イベント受信、解放がそれぞれHTTP timeoutを持ち、解放は最大20回再試行する。たとえば会話timeoutを30秒に設定しても、ローカルProviderの準備待ちだけでそれを超過し、代替先への切り替えが遅れる。

根拠：ready期限（`/Users/y.noguchi/Code/SAAA/src-tauri/src/providers/dynamic_lan/mod.rs:27`）、解決後のtimeout利用（`/Users/y.noguchi/Code/SAAA/src-tauri/src/providers/stream/dynamic_lan.rs:124`）、Agent Session解放再試行（`/Users/y.noguchi/Code/SAAA/src-tauri/src/providers/agent_session.rs:338`）。

改善：SAAAで要求全体のdeadlineとProviderごとの試行予算を設け、discovery・claim・health・推論に残り時間を渡す。解放は独立した短い予算と再試行管理に分離する。LARMは準備中・容量不足・再試行可能時刻を機械判定できる形で返す。受け入れ条件は「準備が終わらないローカルLLMから、指定した予算以内にクラウドへ進む」こと。

### 4. P2：動的LLMを一律JSONに固定する回避策が残っている

allocation IDがあるだけで `RequestMode::JsonTools` を選ぶため、HTTP SSEに対応済みのLARMでも逐次表示されない。コードにはgatewayによるtool/reasoning SSE切り詰めへの回避策とある。ただし、その障害が現在のLARMに残っているかは今回未検証。

根拠：JSONへの固定分岐（`/Users/y.noguchi/Code/SAAA/src-tauri/src/providers/stream/mod.rs:108`）。

改善：LARM側でChat Completionsのtext delta・tool_callsの分割引数・finish_reason・[DONE]を正しく通す。その適合試験を通したうえでSAAAの一律分岐を削除し、対応機能の広告に基づいてSSEを選ぶ。未対応モデルのJSON利用は明示的な能力差として扱う。受け入れ条件は「最初の文字が全文完成前に表示され、分割tool引数も欠落しない」こと。

### 5. P2：TTSのvoice名が特定エンジンに固定されている

LARMが返すmodelとbase URLは利用するが、voiceは常に `Kasukabe_Tsumugi`。新しいTTSモデルがそのvoiceを持たない場合、モデルの差し替えだけで合成要求が失敗する。Provider契約にもvoiceの既定値・候補はない。

根拠：固定voice（`/Users/y.noguchi/Code/SAAA/src-tauri/src/providers/larm_voice/audio.rs:25`）、取得するProvider属性（`/Users/y.noguchi/Code/SAAA/crates/larm-session/src/contract.rs:13`）。

改善：LARMで既定voice・利用可能voice・音声形式を広告し、SAAAで選択・検証する。代替TTSではProviderごとのvoiceを選ぶ。受け入れ条件は「異なるvoice体系を持つTTSへ切り替えても、旧voice名を送らない」こと。

### 6. P2：応答成功後の解放失敗が、会話全体の失敗へ置き換わる

Agent Sessionでは推論がCompletedでも、releaseの再試行が失敗するとInternal failureを新しく返す。ユーザーが受け取った回答と最終状態が矛盾し、本来の出力結果とリソース解放の問題を区別できない。

根拠：releaseによる結果の上書き（`/Users/y.noguchi/Code/SAAA/src-tauri/src/providers/agent_session.rs:132`）。

改善：生成結果を維持し、cleanupの成否を別フィールド・監査記録へ保存する。SAAAで解放を再試行し、LARMではreleaseの冪等性とTTL失効を保証する。受け入れ条件は「応答成功後にreleaseだけ503となっても回答成功を維持し、解放未完了を記録する」こと。

### 7. P2：Providerの接続テスト結果が設定変更後も残る

ProviderCardの接続テストは実行時のProviderで非同期要求を開始するが、設定を変更したときの結果リセットや世代確認がない。URL Aのテスト中にURL Bへ編集すると、遅れて返ったAの成功がBの成功として表示される。テスト完了後にmodelを変えた場合も成功表示が残る。

根拠：無条件で結果を設定（`/Users/y.noguchi/Code/SAAA/src/features/settings/ProviderCard.tsx:44`）。

改善：テスト対象の設定fingerprintと要求世代を保持し、結果反映時に一致確認する。URL・model・認証・credential変更時には結果を未確認に戻す。受け入れ条件は「旧URLへの応答が遅れて到着しても、新URLを接続成功にしない」こと。

### 8. P2：ASRのlanguages応答が言語フィルターに反映されない

ASR応答型は単数の `language` だけを取り出し、未提供時は言語制限の検査を省略する。現行OpenAIのspeech-to-textガイドには `languages: [{code: ...}]` を返す形式がある。この形式のProviderでは、検出言語が返っていても無視され、登録外言語を弾く設定が働かない。

根拠：応答型（`/Users/y.noguchi/Code/SAAA/src-tauri/src/voice/cloud_asr.rs:13`）、languageなしの許可（`/Users/y.noguchi/Code/SAAA/src-tauri/src/voice/language.rs:70`）、[OpenAI speech-to-text](https://developers.openai.com/api/docs/guides/speech-to-text)。

改善：SAAAのASR adapterで対応する応答形式を正規化する。languageとlanguagesの両方がある場合の扱いを定義し、text-only応答では言語が未検証であることを扱えるようにする。受け入れ条件は「許可jaに対しlanguages=[{code:en}]が返った場合、未検証として通さない」こと。

## フォールバックに必要な設計判断

現行のHarness会話ルートは一つの動的Providerだけを返す。直接Providerのルートも `localOnlyWhenSelected=true` ではクラウド候補を除外する。ASRとTTSは単一選択であり、LLM・ASR・TTSの独立した優先順位付きフォールバックは未実装。これらは既存方針であり、WebSocket削除に伴って勝手に緩和していない。

根拠：会話ルートの選択（`/Users/y.noguchi/Code/SAAA/src-tauri/src/providers/routing.rs:6`）。

推奨する設定は、機能ごとのprimary・fallback順序・総deadline・試行予算・localOnly/localPreferredの区別。切り替え先、理由、経過時間、途中出力の有無を可視化する。認証失敗や不正契約は通常の容量不足と分け、同一要求を無条件に繰り返さない。LLMの表示やtool実行開始後は自動再生成しない。TTSも再生開始後の句を最初から別Providerで読み直さず、ASRは同一発話IDで確定を一度にする。

## LARM側に依頼するAPI契約

次は提案であり、現在のLARMがすべて満たすという意味ではない。独自のprofile・claim・lease管理は制御用APIとして残せる。SAAAが推論に使うHTTP APIは、その管理APIから独立したProvider adapterで呼び出せる形にする。

| 機能 | 提案するデータAPI | 必須の互換範囲 |
| --- | --- | --- |
| LLM | POST /v1/chat/completions | model、messages、tools、tool results、JSON応答とSSE。tool引数の分割、終了理由、正常完了と途中切断の区別 |
| ASR | POST /v1/audio/transcriptions | multipartのfile/model、WAV入力、JSON text。任意の言語メタデータは形式を明示 |
| TTS | POST /v1/audio/speech | model/input/voice/response_format。WAVまたは形式を明示したPCMをHTTPで返す。逐次受信・キャンセル |

上記はOpenAI互換を採用する設計提案であり、クラウド全社共通の標準ではない。Claude Messagesなど別schemaのAPIにはadapterが必要。model・voice・tool対応・reasoningパラメータ・最大入力・音声形式の差は能力情報として扱う。

LARMに求める追加条件：

1. claimで機能ごとのHTTP base URL・model・認証方式・対応APIバージョン・能力情報を返す。WebSocketのURLやsubprotocolは必須にしない。
2. 推論要求にallocation IDなどの独自フィールドを必須にしない。割り当てが必要ならclaim由来のURL・credentialで吸収する。
3. busy/unavailable/invalid/authenticationをHTTP statusと安定したerror codeで区別する。再試行可能な場合は待機目安を広告する。
4. readyは制御サーバーの生存確認だけでなく、その機能が推論を受け付けられる状態を意味する。一機能の停止で全機能を利用不能にしない。
5. create/release/renewの冪等性、TTL、失効、キャンセル時の計算停止を定義する。credential更新中に進行中の要求を壊さない。
6. model・voice・対応パラメータが変わったら能力情報も更新する。未対応パラメータを黙って無視せず、クライアントが判定できる応答を返す。

受け入れ試験には、SAAAと独立した通常のHTTPクライアントからの三API呼び出し、LLMの分割SSEとtool実行、ASR text-onlyと各言語形式、TTSの先行再生を含める。さらにローカル容量不足、起動待ち、429/503、途中切断、キャンセル、release失敗、各機能だけのクラウド切り替えを検証する。p50/p95の初回文字・ASR確定・初回音声・フォールバック所要時間を測り、実機・モデル・バージョンを記録する。

## WebSocketについて確認できた事実

WebSocketだからクラウドに接続できない、という一般化はできない。Gemini Live APIはWSSを使う。一方、Claude MessagesはHTTP SSEで逐次応答する。つまり通信方式もイベントschemaもAPIごとに違う。WebSocket必須の独自契約を削除し、HTTP API単位で交換できる構造にする今回の方針は、選択したAPI群への移植性を高める。これは各公式仕様を踏まえた設計判断である。[Gemini Live API](https://ai.google.dev/gemini-api/docs/live-api)、[Claude streaming](https://platform.claude.com/docs/en/build-with-claude/streaming)。

HTTPでもLLMの逐次表示とTTSの音声逐次受信は可能。WebSocketとHTTPのどちらが実環境で速いかは今回測定していない。ASRを音声区間ごとのHTTP送信に統一すると、連続送信と途中結果の更新頻度が変わるため、遅延や途中認識の体験は実測する必要がある。[OpenAI streaming](https://developers.openai.com/api/docs/guides/streaming-responses)、[OpenAI TTS](https://developers.openai.com/api/docs/guides/text-to-speech)。

## 今回削除・更新したもの

- LLMのWebSocket transport、frame処理、ACK/resume/reconnect、旧LARM allocation Providerとgate。
- ASRのnative/harness WebSocket接続とイベント処理。既存のHTTP batch-agreement経路に統一。
- Agent SessionのWebSocket分岐。現在は同一originのHTTP SSEを要求する。Agent Session自体は独自セッションAPIであり、通常のクラウドLLMにはOpenAI-compatible Providerを使う。
- WebSocket専用UI、IPC型・イベント・生成binding、翻訳、専用性能カウンター、専用テスト、旧readinessスクリプトとpackageコマンド、旧WebSocketベンチマーク、旧WebSocket規格書。
- tokio-tungstenite直接依存と不要になったlockfile依存。HTTP Providerに無関係な旧streaming拡張が付いていても無視し、必要なHTTP protocolを検証する。
- kind=larmの保存設定を除去する冪等な移行。旧Providerがprimaryだった場合は未設定へ戻し、既存のクラウドfallbackを勝手にprimaryへ昇格させない。履歴DBの過去値を読めるスキーマは維持する。

残っているWebSocket文字列は、旧形式の拒否・無視を確認する負例、削除チェック、歴史的な記録である。復活用の接続実装やfeature flagは残していない。現在利用するHTTPのLARMセッション・claim管理は残る。

## 検証

- フロントエンド：321 passed / 0 failed。
- Rust lib：515 passed / 0 failed / 13 ignored。
- Rust統合試験：5 passed（IPC生成契約4件、SQLite構造1件）。Rust fixtureを再生成し、フロントの受信schemaとの一致を確認。
- frontend build・Clippy全ターゲット（warningsをerror扱い）・lint・module-size・仕様書検査・差分の空白検査を通過。Viteは500 kB超のchunk警告を出すがビルド成功。
- 追加の全Rust実行時に既存Codex App Server契約テストが一度RequestTimeoutとなった。単独再実行と、その後の全体再実行は成功。タイミング依存の可能性があるため、検証上の注意として記録する。
- 旧設定のみの起動、旧fallbackの除去、HTTP広告に付随する未使用拡張、WSしか広告しないAgent Sessionの拒否を回帰検証。
- 実LARM、外部クラウド、実マイクでの統合動作・遅延は未検証。既存のlive/ignored試験を合格と数えていない。


## SAAA側の後続実装

LARM側への依頼後、SAAAのHTTP adapterと設定・実行経路を更新した。通常のLLM、ASR、TTSには、それぞれ独立したprimary、優先順付きfallback、総時間、1回の試行時間を設定できる。Harnessから個別Providerへのfallbackも利用できる。ローカル限定が有効なときは、保存時と実行時の両方でクラウドへの切り替えを拒否する。

### HTTP APIと設定

- 明示したbase URLのパスを維持する。`/v1beta/openai/`などの末尾へ余分な`/v1`を追加しない。ホストだけのURLは引き続き`/v1`の省略形として扱う。
- LLMの出力上限形式、reasoning parameterの送信可否、tools、HTTP SSEをProvider単位で選択できる。既知のモデル名には控えめな自動判定を行い、別名・独自モデルは設定で上書きする。モデル固有のreasoning値を自動判定できない場合は送らない。
- 動的LLMもSSEを明示的に有効化できる。既存LARMとの適合試験が済むまでは、未指定の動的接続はJSON応答を既定値とする。
- ASRはmultipartのWAV uploadとJSON応答を使用し、`language`と`languages[].code`を統合して言語制限へ渡す。text-only応答を架空の検出言語で補わない。WAV uploadは25 MBまでに制限する。
- TTSはmodel/input/voice/response_formatを送信し、WAVまたはPCMを段階的に受信・再生する。voiceは個別Provider、Harness設定、LARMの広告から取得する。LARMがvoiceを広告せず、利用者の設定もない場合は設定エラーとし、特定エンジンのvoice名を推測しない。
- 設定schemaを15へ更新した。従来の未バージョン指定proxyパスは旧接続先を維持するよう移行し、明示したAPI versionやOpenAI互換のprefixは維持する。移行は冪等で、primaryやfallbackの昇格は行わない。
- 接続テストの結果を設定のfingerprintと要求世代へ結び付けた。URL・モデル・認証・credentialの変更後に、古い成功結果を表示しない。

### 実行経路とフォールバック

LLMは総時間からdiscoveryの経過時間を差し引き、残り時間の範囲でProviderを試行する。Dynamic LANの準備待ちも試行予算で打ち切り、遅れて取得した接続IDの解放はバックグラウンドで継続する。Legacy Dynamic LANの単一試行は既存のlease上限に制限する。

ASRの通常録音と連続入力は同じfallback処理を使用する。連続入力でもHTTP認識要求ごとに保存済み設定を読み直す。LARM音声モードでも個別ASR・TTSの指定が優先される。Harnessを選択した音声turnのLLMだけは、turnごとのDynamic LAN接続を作らず、音声ownerが持つ共有Agent Connectionの`llm` Providerを使用する。テキスト入力とHarness以外の会話Providerは変更しない。

TTSは最初に音声を再生キューへ渡した時点で自動切り替えを止める。再生のコールバックを待たずに判定するため、音声の二重再生を避けられる。一度出力を始めた応答中は、後続の句についても別Providerへの切り替えを行わない。TTSの総時間と試行時間は句単位に適用し、待機中のLLM生成時間は含めない。

自動切り替えの対象は容量不足、利用不能、一時的な上流・ネットワーク障害、タイムアウトなど。認証・契約・ポリシー拒否、キャンセル、LLMの表示・tool実行開始後は切り替えない。ASRの無音や許可外言語も別Providerへ再送しない。試行時間が未指定なら総時間を候補数で等分する。

### LARMのセッションと解放

LARM音声セッションの接続先には保存済みHarness addressを使用する。旧`SAAA_LARM_CONTROL_URL`による別の接続先指定は削除した。音声セッション用profileは設定から指定できる。`SAAA_CONVERSATION_REASONING_MODE=larm`は、Harnessを選択した音声経路で共有leaseを使う互換設定として残る。

音声入力の開始時は所有者だけを登録し、LARMへの接続は必要な機能を初めて使う時点で行う。未使用のASR・TTS・decisionの欠落を理由にclaim全体を拒否せず、healthは要求した機能に対して確認する。一機能のhealth失敗では共有セッション全体を閉じない。アドレス・profile変更時は古いセッションを解放してから再接続する。

音声要求の通信時間は、取得したcredentialの残存有効時間から5秒の余裕を引いた範囲に制限する。設定上のタイムアウトが長くても、leaseの有効期限を越える予算では要求しない。

共有音声turnでは`llm`が会話本文とtool callを生成し、toolの検証・MCP利用・実行・結果返却はSAAAが担当する。27Bのdeltaは画面へ送り続け、確定した最終本文をそのまま既存TTSへ渡す。表示・履歴・最終発話の正本はLLM本文である。

Agent Sessionでは回答生成と解放の結果を別々に保存する。releaseは最大2秒の独立した予算内で再試行し、失敗しても生成成功を取り消さない。キャンセル後も遅れたcreate応答のIDを受け取って解放する。非同期解放の記録は最終観測状態であり、pendingやdeferred-to-ttlを解放成功と扱わない。

### 残る境界と受け入れ確認

管理メモリを有効にしたLLMは、LARM固有の更新確認・整合性契約を使用する。そのため、通常のLLM設定やクラウド自動fallbackへ置き換えない。この制約を設定画面に表示する。memory拡張を伴わない三つの推論APIと、管理メモリの保証は別々に受け入れ確認する必要がある。

SAAAでの自動試験は模擬HTTPサーバーを使う。実LARM・クラウドの認証情報や実音声を使った適合試験は未実施。LARM側の変更後に、能力広告、モデル・voice、SSE tool_calls、lease失効、三機能の独立切り替え、実測遅延を確認する。OpenAI互換API以外のschemaを持つProviderには別adapterが必要である。

### 後続実装の検証結果

- フロントエンド：322 passed / 0 failed。
- Rust lib全体：523 passed / 0 failed / 13 ignored（同時実行4）。その後の調整に対してProvider系68件、音声系116件を再検証した。
- 共有LARM crate：単体15件・HTTP契約4件、reasoning MCP：6件、IPC・SQLite統合：5件が成功。実LARMへ接続するignored試験は実行していない。
- personal-state-core：17件、会話品質の契約60シナリオとruntime試験2件が成功。
- frontend build、format、lint、Clippy全ターゲット（warningsをerror扱い）、module-size、仕様書、差分の空白検査を実施。Viteの既存chunkサイズ警告は残る。
- 全Rust試験を既定の並列数で実行した際、既存Codex App Server契約試験で期待したResponseTooLargeではなくProgressTimeoutとなる事象が一度発生した。同時実行4で全体を再実行し成功した。外部プロセスを使う試験の時間依存として記録する。


## 実機接続検証（2026-09-16）

SAAAを開発しているMacから、保存済みHarness address `http://192.168.0.130:9810`へ通常Bearer認証でHTTP要求した。Profile・claim・Allocation headerは使用していない。認証情報は既存のLARM設定から読み取り、記録には含めていない。送信したのは検証用の短文と合成音声だけで、会話履歴や実マイク入力は使用していない。

稼働releaseは`be5e86272165f4e13e69f4da204b1425486c6070`、検証時のLARM repository HEADは`631c15ad84eacb757770c92b21d647743cfeae6c`。異なるため、受領したsourceベースの報告と配備済み挙動を同一視できない。配備・再起動・設定変更は実施していない。

| 検証 | 実測結果 |
| --- | --- |
| health | 200、release identity取得 |
| 認証 | tokenなしのmodels要求は401、通常Bearerでは200 |
| models | coding-default、decision-default、ASR・TTSを含む8モデルを取得 |
| LLM JSON | coding-defaultとdecision-defaultで「1+1」への本文「2」を確認 |
| LLM SSE | coding-defaultで本文「2」、finish_reason=stop、DONEを確認（70イベント） |
| function tool | connection_checkのtool_callsとJSON引数value=OKを確認。実処理は行わない検証用tool |
| Structured Outputs | 指定したJSON Schemaに合うok=trueの本文を確認 |
| TTS WAV | voicevox-core / Kasukabe_Tsumugi、200、86,572 bytes、24 kHz・mono・16 bit、1.803秒 |
| ASR WAV | 上記音声のmultipart送信で200、「接続を確認しました。」、language=Japanese |
| TTS PCM | 502 upstream_response_format_mismatch。PCM対応はこの配備では合格にできない |
| voices | GET /v1/audio/voices?model=voicevox-coreが400 invalid_request（JSON body要求）。通常の一覧取得は不合格 |

短い英語指示「Reply with just OK.」は、JSON・SSEとも200とstopを返したが本文が空だった。JSONにはreasoning_contentだけが存在した。日本語質問では本文が返るため、LLM通信の全面失敗ではなく、モデル／template／生成設定を含む再現確認が必要な事象として扱う。原因は今回断定していない。

初回のWAV生成は約0.40秒、ASRは約4.63秒、日本語LLM JSONは約10.33秒、SSEは約10.03秒で要求全体が完了した。各条件の単発測定であり、起動待ち・cacheの影響を分離した性能値や初回音声遅延ではない。

今回確認したのは通常HTTP data planeであり、SAAAの画面操作・音声再生まで含むE2E、cloud fallback、障害注入、キャンセル後のengine停止、personal-state保証、Qwen TTS、全モデルの能力を検証したものではない。SAAAのASR言語正規化はJapaneseをjaとして扱える。WAVを使用し、voiceを明示する経路が今回の成功条件である。

追加のtool往復試験では、検証用tool結果ok=trueを返すとmax_tokens=256ではlength終端・本文空になった。max_tokens=1024で再試行するとstop終端・本文「接続チェック成功（OK）」を確認した（約22.25秒）。試験用tool結果を返しただけで外部操作は実行していない。上限変更後の生成は193トークンで終了しており、生成の変動もあるため「常に256では不足する」とは断定しない。
