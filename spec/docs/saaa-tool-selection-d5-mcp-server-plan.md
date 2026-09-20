# SAAA — D4残件解消・D5 MCP公開 実装計画

作成日: 2026-09-20 / v1 / DeepSeek向け実装指示

## 1. 今回の到達点

外部のMCPクライアントからSAAAの `tools_search` → `tools_describe` → `tools_invoke` を呼び、SQLite台帳内のL-Lang能力・外部MCPツールを利用できるようにする。1,500件以上の個別ツール定義をクライアントへ常時渡さない。外部クライアントにも同じ認可・revision固定・条件付き訂正ルールを適用する。

実装順は **D4残件の解消 → D5サーバー → 統合試験 → 自己レビュー**。D6の順位学習、OAuth、リモート公開、stdio、管理UI、World Modelの変更、新たな能力生成は対象外。

参照: [全体計画](saaa-llang-dynamic-capability-m2b-plan.md)、[D4計画](saaa-tool-selection-d4-mcp-implementation-plan.md)、[D4検証報告](verification/tool-selection-d4.md)。旧[transport参考資料](saaa-llang-dynamic-capability-m2b-transport-reference.md)の個別能力のoffer一覧方式は採用しない。D5の契約は本書を正本とする。

## 2. 開始時点と前提

確認時HEADは `848c380`。D4のコードは存在するが、検証報告に未達が残る。特に呼出元futureのabort後のDB終端、接続先URL変更時の訂正ルール保留が未実装と記載され、serviceのinvokeはbackendを直接awaitしている。本計画はD4の品質合格を宣言しない。

作業ツリーにはWorld Model M2等の未コミット変更と計画文書がある。開始時に `git status --short` と対象ファイルのdiffを保存し、既存変更を上書きしない。全体stash/reset/clean、他タスクへのメッセージ送信は禁止。コード実装時に隔離checkoutを使う場合は、必要な未コミット計画文書も参照できるようにし、他作業を取り込まない。

既存の接続点:

| 箇所 | 方針 |
| --- | --- |
| `tool_selection/gateway.rs` | 3入口のschemaとdispatchを再利用。MCP独自の検索・認可を作らない |
| `tool_selection/service.rs` | 実行管理task、明示scenarioによる検索、scope cleanupのAPIを追加 |
| `tool_selection/references.rs` | sessionに割り当てたscopeで参照を分離。TTL10分を維持 |
| `tool_selection/mcp/` | D4の外向きクライアント。D5サーバーを混在させない |
| `src-tauri/src/lib.rs` | 既存serviceを共有するサーバー起動・終了の接続のみ |
| `persistence/` | session用conversationの作成・監査履歴を既存writer経由で保存 |

新規公開サーバーは `src-tauri/src/tool_selection/mcp_server/` に配置する。既存axum依存を利用し、第二のアプリ状態や別の台帳DBを作らない。

## 3. D4残件を完了させるカード

| ID | 作業内容 | 合格条件 |
| --- | --- | --- |
| P00 | 現在のD4試験と基準コマンドを実行し、既存失敗を記録 | 報告書の過去件数を今回実測として転記しない |
| P01 | invokeの受付後は管理taskがbackend実行・取消伝達・DB終端・permitを所有。呼出元abortを検知してもtaskは終端まで動く | barrierでremote送信後にcallerをabort。再起動なしでrunningが消えpermit再利用。実L-Lang停止も回帰試験 |
| P02 | 訂正ルールのremote対象にendpoint_hashを保存し、適用時に現在値と照合 | URL変更で旧rule不適用。別sourceとL-Langのruleは不変。元URLへ戻った際の扱いも明示試験 |
| P03 | D4未達の同名L-Lang＋2MCP振分け、競合、100call満足度、403/redirect、結果容量限界を追加 | D4 A05/A08/A09/A12/A15の残項目を根拠付きで閉じる |
| P04 | 外部descriptor30scenario以上で実E5/BGE評価、1500/10000件測定、自然訂正の会話経路試験 | mock/realを区別。D4 A11/A13と統合回帰を完了、未配備なら未完了を明記 |

P02の具体的保存契約: `tool_selection_rule_source_bindings(rule_id, tool_id, endpoint_hash)` を追加する。ruleのpreferred/avoided等に現れる全remote toolを登録し、1つでもendpoint不一致ならそのrule全体を不適用にする。rule作成・binding作成は同一transaction。既存のremote対象ruleは作成時endpointを立証できないため、migration時に空hashの未確認bindingを作り不適用にする。推測で現在のendpointを割り当てない。

初版は管理UIを増やさず、新しい明示訂正によるrule再作成で有効化する。source更新時にrule_epochも更新して旧参照を失効させる。元URLへ戻っても一度失効したbindingを自動復活させず未確認状態を維持する。新versionは開始時DB version+1とし、World Model等のmigration番号を上書きしない。

P01/P02/P03はD5有効化の前提。P04の準備とサーバー実装は進めてよいが、未完了ならD4/D5全体の完了を宣言しない。

## 4. 設定・認証・公開境界

新規環境変数 `SAAA_TOOL_GATEWAY_MCP_CONFIG` は設定JSONの絶対パス。初版は起動時のみ読み、hot reloadなし。

```json
{
  "formatVersion": 1,
  "enabled": true,
  "port": 43127,
  "tokenFile": "/absolute/private/saaa-mcp-token",
  "projectId": null
}
```

- bindは127.0.0.1固定、endpointは `/mcp`。portは1〜65535、試験内部APIのみ0可。未設定・enabled=falseならlistenerなし。
- tokenFileは絶対パス、内容は32byte以上の乱数をbase64url化したtoken。改行を末尾1つだけ除去し、空白・制御文字を拒否。Unixではowner以外のread/write/execute権限を拒否する。secretを設定例・ログ・DB・応答に保存しない。
- 全methodでBearer認証。token比較はconstant-time。Originがあるrequestは403、Hostは実listenerの127.0.0.1:portだけ許可。CORS許可なし。認証失敗401ではsession/台帳へアクセスしない。
- projectIdはホストに存在するものだけ許可し、全sessionの固定projectにする。nullならuser scopeのみ。principalは現在のローカルprofileから取得。clientInfo、HTTP body、tool引数でprincipal/project/task/conversationを上書きできない。
- tokenはローカルprofileの許可済みツールを呼ぶ資格情報であり、複数の権限主体を区別する機能ではない。別projectの分離は設定と既存ACLで行う。複数token/権限管理は今回増やさない。
- 不正設定・token読込失敗・port競合は公開サーバーだけ無効化し、安全な診断コードを残す。既存会話とD4接続は継続する。自動port変更や全interfaceへのfallback禁止。
- 認可の付与、source登録、L-Lang生成/公開、rule直接編集はMCP toolに追加しない。self接続先 `127.0.0.1:公開port/mcp` と同じendpointをD4 sourceに設定した場合は拒否し、gateway自身への循環呼出しを防ぐ。

## 5. MCP wire契約

採用versionは `2025-06-18`。新旧versionを推測で混用しない。対応範囲は[公式transport](https://modelcontextprotocol.io/specification/2025-06-18/basic/transports)、[lifecycle](https://modelcontextprotocol.io/specification/2025-06-18/basic/lifecycle)、[tools](https://modelcontextprotocol.io/specification/2025-06-18/server/tools)を参照する。以下の上限や認証方式はSAAA独自のローカル接続契約。

Streamable HTTPのサーバー応答はJSONのみ。GET SSE、イベント再送、server initiated requestは提供しない。D4クライアントのJSON/SSE両対応は維持する。

| request | 応答・処理 |
| --- | --- |
| POST initialize | protocolVersion、clientInfo、capabilitiesを検証。対応versionを返し、ランダムsession IDをMcp-Session-Idに設定 |
| notifications/initialized | sessionをReadyへ。202空body。重複は副作用なく受理 |
| ping | 有効sessionなら初期化途中も空result |
| tools/list | Readyのみ。3定義を固定順で返す。nextCursorなし。未知cursorはinvalid params |
| tools/call | Readyのみ。既知の3入口をgatewayへdispatch。事前tools/listは必須にしない |
| notifications/cancelled | 同sessionの型付きrequest IDにだけ取消伝達。未知/完了済IDはno-op、202 |
| GET /mcp | 認証・session検証後405、Allow: POST, DELETE |
| DELETE /mcp | sessionをClosingにし新規受付停止、参照失効、取消開始。204。未知sessionは404 |

initializeはsessionヘッダーなしで受け、返すcapabilitiesは `{"tools":{"listChanged":false}}`。要求versionが異なる場合もサーバー対応版を返す交渉とし、後続のversionヘッダー不一致は400。後続header省略時は有効sessionの交渉版を使う。sessionなしの通常操作は400、未知・失効sessionは404。未Readyの通常操作はJSON-RPC server error `-32002`。

POSTはContent-Type application/json（charset可）、Acceptはapplication/jsonとtext/event-streamを受理できるものを要求する。media typeを文字列完全一致で判定しない。非対応は415/406。body最大64KiB、超過413。envelope parse errorは-32700、JSON-RPC不正/batchは-32600、未知methodは-32601、params不正は-32602。未知notificationにJSON-RPC応答を返さず202とする。

request IDは文字列または整数、型を保存する。`1` と `"1"` は別ID。null・小数は拒否。JSON-RPCエラーはHTTP200で返し、HTTP境界エラーと混同しない。initializeで確保した資源は失敗時に回収し、10秒以内にinitializedが来なければsessionを破棄する。

MCP tools名はproviderと同じ `tools_search`, `tools_describe`, `tools_invoke`。入力schemaはgatewayの共通関数から取得する。JSON schemaの複製を作らない。3定義の合計32KiB以内、説明には取得手順とunknown時の再実行禁止を短く記載する。

tools/callのresultは `content:[{type:"text",text: JSON.stringify(gatewayEnvelope)}]` と `isError` を返す。初版はstructuredContent/outputSchemaなし。未知toolと外側params不正は-32602、既知toolの業務エラーはisError:true。gatewayのok:false、invoke statusがfailed/cancelled/unknown/interruptedの場合はtrue。succeededかつ結果保存不能はfalseのまま既存resultAvailabilityを保持する。boolean falseの成功結果をエラーにしない。wire全体最大40KiBを試験する。

## 6. session・検索scenario・訂正の境界

sessionごとにホスト生成 `conversation_id` と `run_id = mcp-session:<random>` を割り当てる。決定履歴のFKを満たすconversation行を既存SqliteWriter経由で作成する。clientInfoは表示metadataのみで、本人性やscopeの根拠にしない。会話行を作るためだけにユーザー発言を捏造しない。

同一session内でcandidateRef→executionRef→resultRefを共有でき、別sessionでは拒否する。sessionを作り直しても旧参照は再利用不可。TTL10分は維持。終了時はReferenceStore、scenario cache、session関連resultを清掃するが、decision/invocation/訂正の監査履歴は削除しない。conversationの保存期間は既存履歴方針に従う。

検索にはscenarioが必要だが、現 `begin_turn` は訂正抽出と保存も行う。MCPのintentをそのままbegin_turnへ渡してはいけない。外部エージェントの検索文字列をユーザーの訂正として記憶する危険がある。

`extract_scenario_only(intent)` と `search_with_scenario(context,intent,limit,scenario)` を追加する。既存抽出器・validatorを再利用しつつ、訂正候補は保存せず捨てる。ホストscopeはcontextからのみ設定し、抽出失敗はScenario::degradedで明示する。searchではrequest-localなscenarioを渡し、session共有cacheを書き換えない。同sessionで異なるintentの並列検索が混ざらないことを試験する。

D5は既存のuser/project scopeの訂正を検索へ適用する。MCP経由の新規feedback受付は今回対象外。SAAA会話に保存されたユーザー訂正の通常経路を維持し、MCPのintent・tool結果・無反応をfeedbackとして記憶しない。sessionのconversation限定ruleは他の通常会話へ転用しない。この制限をクライアント向け説明に明記する。

## 7. 実行所有権・重複・終了

session最大16、idle TTL30分、in-flight中はidle失効させない。session内tools/call最大4、全体16。サーバー側待機queueなし、満杯は送信前busy。認証前のbody読み込みもtimeout10秒で制限し、無制限に待機させない。

`(session_id, typed_request_id)` に管理task・取消handle・入力hash・応答を紐付ける。tools/callを受理した時点でIDを予約し、同時重複は-32600で拒否する。完了したIDを同sessionで再利用した場合も再実行せず拒否する。ID履歴はsession最大4096件、満杯なら新規requestを拒否しsession再作成を要求する。メモリ節約のため履歴を捨てて副作用を再実行しない。

HTTP応答receiverが閉じても管理taskをabort/cancelしない。TCP切断は取消ではない。明示取消・DELETE・deadline・アプリ終了だけが取消の起点。P01の「caller dropで取消」と衝突しないよう、HTTP handlerとservice callerの間に継続する管理taskを置き、handler dropでservice futureをdropしない。

同IDによる結果再送機能は初版に含めない。切断したクライアントは「実行結果不明」と扱い、別IDで自動再試行しない。JSON-RPC ID重複防止は同sessionの限定保証であり、別session・新IDでのexactly-onceを保証しない。監査履歴には完了結果を記録する。

各tools/callの全体deadline30秒。下位backendの既存timeoutを延長しない。deadline後は取消を伝達して管理taskに終端を任せる。remote停止未確認はunknown、実L-Lang停止確認済みならcancelled。DB finishは1回、permit解放も1回。task panicも監督taskが拾って技術状態を終端させる。

終了順: listener受付停止→session Closing→全call取消→最大5秒待機→残remoteをunknownで記録→必要なworker終了確認→session/参照/result cleanup→join。結果保存用writerとD4 managerはD5管理taskの処理が終わるまで保持する。途中でDBを閉じない。ローカルprocessを強制終了する場合は既存停止処理に委譲する。

## 8. D5実装カード

各カードを実装→指定試験→差分確認の順に終える。名前や責務を自由に増やして計画を巨大化させない。

| ID / 依存 | ファイル・作業 | 合格条件 |
| --- | --- | --- |
| S01 / P00 | `mcp_server/config.rs`。設定・token・bind検証 | 未設定でlistener0、不正設定で会話継続、token露出0 |
| S02 / S01 | `protocol.rs`。型付きID、envelope、method、error mapping | ID型、batch、notification、false成功、unknownを独立fixtureで試験 |
| S03 / S02 | `sessions.rs`, `context.rs`。状態・TTL・scope・conversation作成 | FK正常、別session参照拒否、上限と未initialized回収 |
| S04 / S03 | serviceのscenario-only APIとrequest-local検索 | intentに「次回から使うな」を入れてもrule増加0、並列intent非干渉 |
| S05 / P01,S03 | `calls.rs`。管理task・ID予約・取消・監督 | HTTP切断で継続、明示cancelで停止要求、重複callの下位実行0 |
| S06 / S02,S04,S05 | `router.rs`。POST/GET/DELETE、認証、3入口dispatch | 実HTTPでinitializeからresult pageまで完走 |
| S07 / P02,P03,S06 | `mod.rs` とlib起動/終了配線、self接続拒否 | 不正公開設定でもD4/会話継続、アプリ終了でrunning残存0 |
| S08 / S07 | `tests/`。下記H01〜H14、既存D4/D0〜D3回帰 | barrierによる競合試験、実HTTP、実L-Langを含む |
| S09 / P04,S08 | 独立MCPクライアントのsmokeと性能測定 | 手書きHTTP試験だけでなく独立クライアントで3入口を確認 |
| R1 / S09 | 初回snapshotを固定し全差分レビュー | 再現条件・影響・根拠行・修正案。mdの完成度を品質根拠にしない |
| R2 / R1 | 修正、該当試験、全体gate、結果報告 | 未達を明記、重大な認可/二重実行/履歴欠落を残さない |

実装は `start(service, writer, config) -> ServerHandle` のようにテストから起動できる形にする。module importでlistenerを起動しない。公開listenerは既存serviceのArcを受け取り、FixtureBackendを本番へ配線しない。

## 9. 受入試験

| ID | 操作と期待値 |
| --- | --- |
| H01 | 1500tools同期後tools/listは厳密に3件。台帳全schema/使い方をinitializeやinstructionsへ混入させない |
| H02 | 認証なし/不一致、Origin、Host不一致を全methodへ送る。台帳読取・backend callとも0 |
| H03 | session欠落/未知、初期化前call、version不一致、batch、不正ID、大小body、Acceptパラメータを検証 |
| H04 | L-Langと外部MCPに同名tool。search→describe→invokeで指定先のみ実行、権限外は全経路拒否 |
| H05 | 別sessionへcandidate/execution/resultRefを転用。全拒否。同session内は正常 |
| H06 | 同一ID並列2call、完了後同ID、整数1/文字列1。二重実行なし、型別IDは別requestとして動く |
| H07 | HTTP切断後に下位実行をbarrier解除。DBは正常終端。同時に別sessionの同IDをcancelしても巻き込まない |
| H08 | 明示cancel、DELETE、deadline、caller abort、shutdown、task panic。終端/permit/参照cleanupを確認 |
| H09 | session16件、call4/16、ID4096件、TTL/未初期化timeout。上限超過は下位call0、既存実行を追い出さない |
| H10 | 100KiB日本語結果の全page復元。別session/ACL撤回/期限切れ拒否。wire40KiB以内 |
| H11 | 同sessionで異なるscenarioを同時検索。互いのintent・scopeが混入せず、intentから訂正ruleが作られない |
| H12 | 通常会話で保存したproject/user訂正が該当MCP検索だけに適用。別project・別operation・endpoint変更後は不適用 |
| H13 | 連続100回の成功/失敗/unknownでも満足度positiveが増えない。障害時のエラーにtoken/path/SQLを露出しない |
| H14 | 独立クライアントでinitialize/list/search/describe/invoke/大結果/終了。再起動で旧sessionと参照は無効 |

ネットワーク試験はloopbackの実HTTPを使用し、外部有料サービスを前提にしない。時間は注入clock、競合はbarrierで制御する。実ML評価はP04と分離し、mockで通過したものを実モデル精度の根拠にしない。

## 10. 検証・提出

開始時と最終時に以下を実行し、終了コードと失敗原因を記録する。

```sh
cargo test --manifest-path src-tauri/Cargo.toml --lib tool_selection
cargo test --manifest-path src-tauri/Cargo.toml --lib generated_capabilities
cargo test --manifest-path src-tauri/Cargo.toml --lib providers
cargo fmt --check --manifest-path src-tauri/Cargo.toml
cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings
bun run size:check
```

最終は `bun run check` も実行。別作業由来の失敗はファイル・再現コマンドを示して分離するが、未実行を通過としない。size上限緩和、試験削除、ignored化で合格させない。モデル/独立クライアント依存はversionを固定し、再現コマンドを報告に記す。

自己レビューは「認証→session→context→scenario→search→revision参照→認可→実行→終端」と「HTTP切断→管理task継続→DB終端」をファイル横断で追う。さらに、訂正binding migration、endpoint変更、並列search、DELETEとdispatchの競合を反例で確認する。

成果物は実装・試験・ `spec/docs/verification/tool-selection-d5.md` 。開始/初回/修正後snapshot、P/S/H各項目の状態、migration、実測値、自己レビュー指摘と修正、未達を記載する。D4残件もこの報告から追跡できるようにする。

## 11. 担当AIへの依頼文

> 本計画のP00〜P04、S01〜S09、R1/R2を実装してください。まずD4のabort所有とendpoint変更時の訂正記憶を修正し、SAAAの既存3入口をローカルMCPサーバーとして公開してください。既存gateway・ACL・参照を再利用し、sessionをホストscopeへ結び付けてください。HTTP切断を取消と扱わず、検索intentをユーザー訂正として保存しないでください。未コミットの他作業を保護し、初回snapshotを残して自分で全差分をレビュー・修正・再検証してください。D6や別機能へ進まず、実測と未達を正確に報告してください。
