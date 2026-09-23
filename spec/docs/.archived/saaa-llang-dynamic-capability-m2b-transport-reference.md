# M2B旧案 — HTTP transport参考資料

この文書は2026-09-20改訂前の計画を保存した参考資料です。実装指示の正本は[M2B改訂計画](saaa-llang-dynamic-capability-m2b-plan.md)です。以下の直接gc_公開、8件の登録制限、listのoffer保存、boolean限定のgateway入力は新計画へ持ち込まないでください。HTTP認証・セッション・所有task・取消の扱いだけを参照し、矛盾時は新計画を優先します。

## SAAA × L-Lang — M2B ローカルMCP公開の実装計画

作成日: 2026-09-20

状態: 実装指示。今回作成したものは計画書であり、M2B実装・合格を意味しない。

上位文書: [初期計画](saaa-llang-dynamic-capability-initial-plan.md)、[M2A計画](saaa-llang-dynamic-capability-m2a-plan.md)

## 1. 目的と完成条件

SAAA内で検証・有効化・公開されたL-Lang能力を、外部のローカルMCPクライアントから列挙・実行できるようにする。会話経路と同じcatalog、公開allowlist、CapabilityService、検証済みruntimeを使う。

完成例は `SAAA起動 → 認証付きinitialize → initialized → tools/list → gc_<revision>をtools/call → boolean結果を受信`。停止・revision更新・取消・切断・アプリ終了でも、別revisionへ勝手に差し替えず、実行枠と子プロセスを管理する。

今回実装するのはM2Bのみ。LLMによる生成、管理UI、import/verify/activateのMCP公開、TypeScript inspection、新しいABI・effect、remote公開、OAuth、stdio、SSE配信、listChanged通知は対象外。MCP全機能対応とは表現しない。

## 2. 作業開始時点の扱い

現在の作業ツリーにはM2Aコードと、別作業のpersonal-world-model変更がある。未コミット・未追跡コードも含むため、HEADとの差分だけを今回の成果と数えない。M2Aの実装が存在することと合格は別であり、G0で入口検査を行う。

変更前のHEAD、status、関連ファイルのコピー・SHA-256、追跡差分、新規ファイル一覧をrepository外へ保存する。秘密情報・DB・target・node_modulesは保存しない。別作業の変更を消すreset/clean/stash、無断の一括整形、world-model側の修正はしない。

この文書で「新規」と書いた型・設定・ファイルは今回の設計指定。実装済みではない。

### 最初に読む実コード

| repository rootからのパス | 確認する責務 |
| --- | --- |
| `src-tauri/src/generated_capabilities/publication.rs` | GeneratedToolsConfig、GeneratedToolSnapshot、descriptors、上限 |
| `src-tauri/src/generated_capabilities/tools.rs` | 共通execute、origin、RunCancellation、安全な結果JSON |
| `src-tauri/src/generated_capabilities/service.rs` | resolve_publication、invoke、受付とshutdown |
| `src-tauri/src/generated_capabilities/execution.rs` | 子処理・permit・DB終端の所有者 |
| `src-tauri/src/generated_capabilities/contracts.rs` | ResolvedCapabilityとInvokeRequest。旧計画のservice.rs内型配置を前提にしない |
| `src-tauri/src/generated_capabilities/tests/{adapter_abort,hanging,invoke_abort}.rs` | 実行開始を同期した取消試験 |
| `src-tauri/src/{app_state,lib,test_state,test_support}.rs` | 共有状態・起動・終了・テスト初期化 |
| `services/reasoning-mcp/src/lib.rs` | 既存Axum HTTP境界の参考。固定reasoningツール専用の制約はコピーしない |

現状のtools::executeはAgentToolCallとRunCancellationを受け取りStringのJSONを返す。MCPから薄い変換で利用してよい。公開APIを全面改造したり、別CapabilityServiceを構築したりしない。publicationのdefinitions()はparametersキーを使うため、そのままMCPへ送らずdescriptors()からinputSchemaへ変換する。

## 3. 実装するMCPの範囲

プロトコル版は既存reasoning MCPと合わせて **2025-06-18** に固定する。これは採用版の指定であり、最新版との主張ではない。Streamable HTTPのJSON応答のみとし、セッションを持つ。仕様の根拠は[transport](https://modelcontextprotocol.io/specification/2025-06-18/basic/transports)、[lifecycle](https://modelcontextprotocol.io/specification/2025-06-18/basic/lifecycle)、[tools](https://modelcontextprotocol.io/specification/2025-06-18/server/tools)、[cancellation](https://modelcontextprotocol.io/specification/2025-06-18/basic/utilities/cancellation)。以下の上限・認証・offer保持はSAAA独自の制約である。

### 3.1 設定と起動

新規環境変数 `SAAA_GENERATED_MCP_CONFIG` は設定JSONの絶対パス。起動時に一度読み、hot reloadはしない。

```json
{"formatVersion":1,"enabled":true,"port":43127,"tokenFile":"/absolute/private/generated-mcp-token"}
```

- 未設定・enabled=falseならlistenerを作らない。不正設定・token読込失敗・port競合はMCPだけ無効化し、SAAAと会話機能は継続する。
- 未知キー/version、相対path、port=0または65536以上、64 KiB超の設定は拒否。テスト内部の起動APIだけport=0を許す。
- bindは127.0.0.1固定。設定にhostキーを作らない。自動port変更・全interfaceへのfallbackは禁止。
- tokenFileは32〜256文字の可視ASCIIトークン一つ。末尾LFまたはCRLF一つだけ除去し、内部空白・改行は拒否する。ファイルは上限付きで読み取る。例にはダミー値だけを使う。
- `Authorization: Bearer <token>`を必須にする。providerのAPI keyやreasoning MCP tokenを流用しない。token本文・Authorization・設定本文をログやLLMへ渡さない。比較には長さと全バイトを処理し、先頭一致で早期成功しない。
- これはローカル向け固定Bearer認証であり、MCPのOAuth discovery対応を名乗らない。
- 公開能力は既存SAAA_GENERATED_TOOLS_CONFIGのallowlistとenabledを共有する。MCP独自allowlistは作らない。公開無効ならtools/listは空、tools/callは拒否。

### 3.2 HTTPとJSON-RPC

endpointは `/mcp` 一つ。POSTで単一JSON-RPCメッセージを処理する。GET/DELETEの方針を下表で固定する。POSTはapplication/jsonを要求し、応答はJSON。MCPクライアントのAcceptはapplication/jsonとtext/event-streamを受け入れる形式とする。メディア型のパラメータを含む正当な表現も扱う。

認証なし・不一致は401、Originヘッダが存在すれば403（ブラウザ接続は今回対象外）。認証・Origin検査は全メソッドで実施し、失敗時はcatalogを読まない。HTTP body上限64 KiB、超過413、Content-Type不一致415、Accept不一致406。CORS許可ヘッダは付けない。

| 操作 | 採用する動作 |
| --- | --- |
| initialize | protocolVersion/clientInfo/capabilitiesを検査。対応版が違えば対応版2025-06-18を返し、クライアント側の互換判定を可能にする。serverInfo.name=`saaa-generated-capabilities`、capabilitiesはtools.listChanged=falseのみ |
| 初期化応答 | 暗号学的乱数を使うUUID v4をMcp-Session-Idとして返す。セッションはメモリ内だけ |
| notifications/initialized | 対応セッションをreadyへ。202空body。二重通知も202 |
| ping | 有効セッションなら初期化通知前もresult={} |
| tools/list | readyのみ。最大8件なのでpaginationしない。非空cursorはinvalid params |
| tools/call | readyのみ。nameとargumentsを検査して共通adapterへ。JSON-RPC IDをDB主キーにしない |
| notifications/cancelled | 同一セッションのrequestIdだけを取消。202空body。未知・終了済みIDは何もしない |
| GET /mcp | 認証後405。SSE未提供 |
| DELETE /mcp | セッションを受付対象から外し、そのセッションの処理を取消・回収して204。未知セッションは404 |
| その他のrequest method | -32601。sampling/prompts/resources等を宣言しない |

初期化後はセッションIDを要求する（欠落400、未知・期限切れ404）。MCP-Protocol-Versionは指定された値が非対応なら400、欠落時はセッションで交渉済みの版を使う。initializeに既存セッションIDを付ける曖昧な再初期化は400で拒否する。

request IDは文字列または整数で、型を保持する（`1`と`"1"`は別）。null・boolean・小数は拒否。文字列はUTF-8で128バイト以下。同一セッションの処理中IDの重複は拒否し、既存処理を置換しない。配列batchは拒否。JSON構文エラーは-32700、不正envelopeは-32600、methodの引数不正は-32602。IDを安全に読めない場合だけerror応答のid=null。

有効な通知にはJSON-RPC応答を生成しない。未知通知は202で無視する。サーバーからrequestを送らないため、クライアントからのresponseは400で拒否する。JSON-RPC envelopeを厳密にしつつ、MCPの正当な任意フィールド（_meta等）を一律deny_unknown_fieldsで破壊しない。

### 3.3 セッションの上限とoffer

最大16セッション、idle TTL30分、満杯時initializeは503で拒否し、既存セッションを追い出さない。idleはin-flightなしの状態から数え、実行中にTTL失効させない。掃除はrequest時と60秒周期で行う。受付後のtools/callには共通adapterの15秒上限を適用する。サーバー独自の実行待ちqueueは作らない。

tools/listごとに一度のresolve_publicationからsnapshotを構築し、同セッションの直近offerとして保存する。応答のname/description/inputSchemaと保存snapshotを同時に確定する。各セッションにsnapshotは一つだけ。一覧作成失敗時は古いofferを消して内部エラーを返す。inactive項目の除外はM2Aと同じ。

tools/callは保存offerから名前を解決し、cloneした不変snapshotを共通executeへ渡す。tools/list前のcallはinvalid params。未offer名・古いrevision名・allowlist外の呼出しも拒否する。実行時の状態/epoch/hash再確認は既存serviceに任せ、最新版へ再解決しない。並行tools/listが保存offerを更新しても、既に受付済みのcallが持つsnapshotは変えない。

入力16 KiB、定義32 KiB、boolean必須・追加キー禁止などのM2A制約を維持する。MCP定義ではparametersをinputSchemaへ変換し、実際のMCP tools配列をserializeしたサイズも検査する。超過を黙って切り詰めない。

### 3.4 結果とエラー

成功は `result.content=[{"type":"text","text":"共通adapterが返したJSON"}]`、isError=false。boolean falseも成功。今回structuredContent/outputSchemaは追加しない。

未知ツール名・不正argumentsはJSON-RPC invalid params。既知ツールのstale/busy/timeout/cancelled/integrity/storage等の実行失敗はresult.contentに安全な共通エラーJSONを入れisError=true。共通adapterのStringをparseする変換は一箇所だけに置き、不正結果を成功扱いしない。originは`mcp`を指定する。path・stderr・SQL・tokenを返さない。

## 4. 取消と所有権で間違えてはいけないこと

HTTP切断だけをMCPの明示取消と同一視しない。MCP callはサーバー所有taskで実行し、handlerのresponse待機がdropされてもtaskは既定deadlineまで所有し続ける。前段のhandler破棄で共通adapter futureまで破棄してしまう構造は禁止。

`(session ID, 型を保持したrequest ID) → RunCancellation/処理handle`を管理する。cancel通知、DELETE、サーバー終了、timeoutで取消を伝える。共通adapterはRunCancellationからhost取消へ接続済みなのでそれを使い、host直呼出しを作らない。

taskが結果保存・子プロセス回収・permit解放を終えた後、必ず処理mapから除去する。error早期return、handler切断、panic/JoinErrorも確認する。セッション全体のmutexをawait越しに保持しない。HTTP受付の上限が埋まってもcancel/deleteが処理できる経路を確保する。

同時実行枠は会話とMCPで共通の既存1枠。取消mapを別枠の実行許可に使わない。同じrequest IDを別セッションで使っても互いに取消されない。完了済みrequestの自動再実行・結果replay cacheは今回なし。クライアントはセッション内のrequest IDを再利用しない。

アプリ終了では `MCPのtools受付停止 → 所有call取消・await → listener/管理task終了 → 既存CapabilityService終了` とする。既存sync shutdownを非同期executor上で呼んでdrainを塞がない。新規McpRuntimeにasync shutdownを持たせ、lib.rsの終了フローへ接続する。終了失敗を成功ログに置換しない。

## 5. 実装カード（この順序を守る）

| 工程 | 作業 | 完了条件 |
| --- | --- | --- |
| G0 | 開始snapshotを保存しM2Aのpublication/adapter/provider/abort試験を実行 | 件数0を合格にしない。基盤不具合なら関連修正と再検証を先に行い、別作業の失敗と分ける |
| B1 | 新規`generated_capabilities/mcp/config.rs`、設定検査 | 設定・秘密情報・disabledの試験合格 |
| B2 | 新規`mcp/protocol.rs`と`mcp/session.rs` | envelope、型付きID、ready、TTL、上限、offer管理の試験合格 |
| B3 | 新規`mcp/router.rs` | 認証/Origin/body制限、initialize/list/call/cancelを実HTTPで確認 |
| B4 | 新規`mcp/runtime.rs` | bind、所有task、取消、停止、join。実PIDを観測する取消試験合格 |
| B5 | AppState/lib/test初期化へ配線 | 同じArcと公開設定を使用し会話/MCP間の共通capacityを確認 |
| R1 | 実装完了時snapshotを保存して自己コードレビュー | 下記の反例試験とfile:line付き指摘。修正前の成果を上書きしない |
| R2 | 指摘修正・再検証・提出 | 未解決の今回契約違反を残して完了扱いにしない |

既存axum/tokio/serde/uuid/reqwestを使う。新規MCP SDKやcrate分割は今回不要。reasoning-mcpは参照し、固定ツール、UUID限定request ID、別DBなどの制約を移植しない。moduleが大きくなる前に上表の責務で分ける。新規fileのサイズ登録は可、既存閾値の引上げで回避しない。

## 6. 必須試験

テストserverは127.0.0.1:0で起動し、実際のHTTPをreqwestで送る。外部LLM・有料API・本番tokenは不要。server/taskの終了をテスト側でもawaitし、試験後にport・実行登録を残さない。

| ID | 試験と合格条件 |
| --- | --- |
| H01 | 設定なし/disabled、不正JSON/相対path/port/tokenでlistenerなし。会話は使用可 |
| H02 | 全methodで認証拒否・Origin拒否。失敗時catalog/host呼出しゼロ。tokenが応答・ログにない |
| H03 | body/Content-Type/Accept境界、GET405、未知path404 |
| P01 | initialize→initialized→list→callが通る。未知versionは対応版返却、非対応HTTP版は400 |
| P02 | セッションなし/未知/未ready、TTL、16件上限、DELETEを固定契約通りに処理 |
| P03 | string/integer IDを保持、重複in-flight IDを拒否。batch/null/不正JSONも拒否 |
| P04 | 通知202空body。未知通知無視、cancel不能なIDは他callへ影響なし |
| T01 | allowlist内activeだけ一覧に出る。inputSchemaにboolean制限、32 KiB境界を検査 |
| T02 | 正規fixture Aでtrue/falseを実行。DB origin=mcp、内部UUID、isError=false |
| T03 | 不正引数・未offer・偽造名でhost起動ゼロ。recall等へfallbackしない |
| T04 | list A後にB有効化でAを拒否。次listはB、AのcallをBへ差替えない |
| T05 | list後suspend、runtime/package改変を拒否。安全なエラーだけを返す |
| C01 | 実子プロセスがPID開始通知を出すまで待ちcancel。DB cancelled、PID消滅、登録0、枠再利用 |
| C02 | 別sessionで同じrequest IDを使い、一方のcancelが他方を取消しない |
| C03 | 実行中HTTP接続切断だけでは即cancelしない。明示cancelまたは期限で回収される |
| C04 | timeout、DELETE、アプリ終了で実子処理を回収。終了後新規callを受付けない |
| C05 | 会話実行中のMCP callと、その逆が共通capacityに従う。busy中もcancelを受付ける |
| C06 | DB終端書込失敗・handler drop・task異常で成功を偽装しない。登録とpermitの所有者を確認 |
| R01 | M2Aの会話・更新・取消テスト、旧wasm_host_pocが引き続き通る |

「2ms sleepしてabort」「DBがrunningでない」だけではC01合格にしない。開始を観測し、cancelledを指定して検証する。実行前に落ちてcallが存在しない場合や、最後までsucceededになった場合は失敗である。少なくともT02/T04は正規runtimeを使い、全試験をfake hostで済ませない。

## 7. 自己レビューと提出

開始状態→初回実装→自己修正後の3状態を区別する。未追跡コードも全文読む。Markdownの出来を実装品質に数えない。

一件のcallについて `HTTP認証 → セッション → offer → adapter → service → 子処理 → DB終端 → MCP応答` を追い、各境界のfile:lineを記す。特に他セッション取消、handler切断時の所有者、mutexを跨ぐawait、停止と新規登録の競合、古いoffer再解決、token漏出、falseのエラー誤認を確認する。

指摘は `重要度 / 発火条件 / 期待と実際 / 根本原因 / 再現テスト / 修正 / 再検証`。不具合は修正前に失敗し修正後に通る試験を残す。テスト期待値を誤動作へ合わせない。

最低限の検証コマンド:

```sh
cargo test --locked --manifest-path src-tauri/Cargo.toml --lib generated_capabilities
cargo test --locked --manifest-path src-tauri/Cargo.toml --lib providers
cargo test --locked --manifest-path src-tauri/Cargo.toml --lib wasm_host_poc
cargo fmt --check --manifest-path src-tauri/Cargo.toml
cargo clippy --locked --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings
bun run size:check
bun run check
```

新規MCPテストがこれらのfilterに含まれることを件数と名前で確認する。外部環境依存の未実施は明記し、無関係な既存失敗をこの作業で大改修しない。

提出する検証報告は `spec/docs/verification/llang-dynamic-capability-m2b.md`。対応試験ID、実行件数・失敗、初回/修正後snapshotの場所、自己レビュー結果、設定例、curlまたはreqwestでの完全なinitialize/list/call例を記載する。接続手順にはローカル固定Bearer・対応版・セッション・list先行が必要な制約を明記する。

## 8. 担当AIへ渡す指示

> `spec/docs/saaa-llang-dynamic-capability-m2b-plan.md`に従い、G0からR2まで実装してください。現在の未コミットM2Aと別作業の変更を保護し、開始状態を保存してください。同一CapabilityServiceとM2A共通adapterを使って認証付きローカルMCPを接続してください。独立したhost実行経路は作らないでください。実HTTPと実子プロセスの開始確認を含む試験を実施し、初回実装snapshotを保存してから自己コードレビューしてください。問題は再現・修正・再検証まで行い、未実施を合格と報告しないでください。M3、LLM生成、UI、remote公開へ進まず、今回のM2Bを完成させてください。
