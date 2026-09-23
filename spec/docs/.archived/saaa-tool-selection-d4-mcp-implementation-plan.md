# SAAA — D4 外部MCP接続・台帳同期 実装計画

作成日: 2026-09-20 / v1 / DeepSeek向け実装指示

## 1. 今回の到達点

外部MCPサーバーから取得した1,500件以上のツールをSQLiteに同期し、既存の3入口から検索・説明取得・実行できるようにする。通常のユーザー訂正は、外部ツールについても、そのシナリオに限って次回の順位を変更する。

実装対象はD4だけ。SAAA自身をMCPサーバーとして公開するD5、Bandit等の学習D6、L-Langによる新規能力生成、World Modelの拡張は今回に含めない。まず外部MCPを使える状態を完成させ、その後でSAAAを他のエージェントから使う入口を作る。

上位契約は[M2B全体計画](saaa-llang-dynamic-capability-m2b-plan.md)、既存検索・訂正の契約は[D0〜D3詳細手順](saaa-tool-selection-d0-d3-implementation-guide.md)。D4の具体的な境界・上限・カード順は本書を正本とする。ここで指定する上限はSAAAの実装判断でありMCP仕様の上限ではない。

## 2. 開始時点と変更が必要な箇所

計画作成時HEADは `f5a1069`、DB schema versionは23。作業ツリーに変更なし。D0〜D3の実装と検証報告が存在することを確認した。本書は品質レビューの合格証ではない。開始時にT00の試験を実行する。

| 現コード | D4で必要な変更 |
| --- | --- |
| `tool_selection/mod.rs::build_service` がLlangBackendを直接構築 | source種別で分岐するBackendRouterとMCP管理サービスを構築 |
| `schema.rs` のsources.kindがllangだけ | 既存DBを保ったmigrationでmcp_httpを追加 |
| `catalog.rs::register_revision` がllang固定、1件ごとepoch更新 | source作成とrevision登録を分離し、一括同期でepochを1回更新 |
| `service.rs::ingest_catalog` はrev1固定かつgrantを追加 | 外部MCP同期には流用しない。同期と認可を別APIにする |
| `repository.rs::tool_id_by_name` はLIMIT 1 | source間の同名衝突を曖昧として扱う |
| `feedback.rs::resolve_tool_id` が名前解決を使用 | decision内の候補・認可済み対象から一意に解決 |
| `service.rs::invoke` はbackendを直接awaitし16KiB超を失敗扱い | 外部副作用の不確定状態と、成功した大結果のページ取得を追加 |

既存ファイルを大きくまとめ直さない。新規モジュールを `tool_selection/mcp/` に分け、既存ファイルでは境界の変更を行う。既存のL-Lang process管理、実行取消、World Model処理を削除・置換してはならない。

## 3. 固定する公開範囲と設定

初版transportはStreamable HTTP、protocol versionは `2025-06-18` に固定する。最新版であるという意味ではなく、試験する互換範囲の固定である。stdio、旧HTTP+SSE、OAuth、自動サーバー探索、sampling・elicitation・roots・resources/readは実装しない。未対応のserver requestには対応するJSON-RPCエラーを返し、実行しない。

接続先はホスト管理設定からのみ登録する。LLMからURL・token・source・grantを指定させない。既存 `SAAA_TOOL_SELECTION_CONFIG` に任意フィールド `mcpSourcesPath` を追加し、絶対パスの別JSONを読む。未指定なら既存挙動。設定formatVersionは1を維持し、省略時互換を試験する。

```json
{
  "formatVersion": 1,
  "sources": [{
    "id": "mcp-office",
    "url": "http://127.0.0.1:8787/mcp",
    "enabled": true,
    "bearerTokenEnv": "SAAA_OFFICE_MCP_TOKEN",
    "grants": [{"toolName": "search_notes", "scopeKind": "user"}]
  }]
}
```

設定契約:

- source idは `[a-z0-9][a-z0-9_-]{0,63}`、重複不可。sources最大32、ファイル最大1MiB。token指定は任意。指定envが空・未定義ならそのsourceを接続不能にする。
- URLはHTTPS、またはHTTPの `127.0.0.1` / `[::1]` のみ。userinfo・fragmentは禁止。redirectは追跡しない。proxy環境変数を暗黙使用しない。TLS検証を無効にしない。
- owner principalは既存ローカルprofileから設定しJSONには書かせない。project grantは `scopeKind:"project", projectId:"ホストに実在するID"` を要求する。user grantのscope_idもホストで補う。wildcard grantなし。
- grantsはこの設定が管理する認可の宣言。未登録toolNameへの宣言は保留し、同期で名前が現れた後に独立した認可transactionで適用する。台帳import自体はgrantを作らない。
- JSON全体の構文・重複ID・未知フィールド・型不正は設定全体を拒否。初回ならMCPを起動しない。再読込失敗なら直前の有効設定を保つ。状態には診断コードだけ表示しsecretを残さない。
- 管理API `reload_config()`、`sync_source(source_id)`、`status()`、`shutdown()` を設け、既存アプリの起動・終了から接続する。初版の設定変更反映は再起動でもよい。LLM用toolを追加しない。
- sourceを削除・無効化した場合は、新規dispatchを停止し、台帳のsourceをdisabledにしcatalog_epochを更新する。設定由来のgrantを取り消しacl_epochを更新する。他の管理経路で付与したgrantは削除しない。

## 4. 永続化・ID・同期の契約

DB versionは開始時の最新値+1にする（他変更がなければ24）。`CREATE TABLE IF NOT EXISTS` の変更だけで既存CHECK制約を更新したことにしない。sources再構築は既存migration経路のtransaction・FK規約に従い、参照先とデータを保存する。migration後に `foreign_key_check` が空であること。

追加テーブルは次の最小構成とする。全て `tool_selection_` 接頭辞を付ける。

| テーブル | 必須列・制約 |
| --- | --- |
| mcp_sources | source_id PK/FK sources、config_generation非負、endpoint_hash、last_success_at nullable、last_error_code nullable、published_generation非負 |
| mcp_managed_grants | source_id、principal_id、tool_id、scope_kind、scope_idの複合PK。設定由来のgrantを追跡。既存手動grantを誤って所有しない |
| mcp_results | id PK、invocation_id FK、principal_id、conversation_id、scope_key、tool_id、revision_id、schema_hash、acl_epoch、expires_at、payload_json、byte_count。payload最大1MiB |

接続session ID・token・HTTPヘッダー・SSE本文・生のエラー本文をDBへ保存しない。source URLは設定側のみ。endpoint_hashはsecretを除いた正規化URLのSHA-256。revision bindingにtoken/URLを埋め込まない。

ID規約をコードと試験で固定する:

- tool_id = `mcpt_` + SHA256(canonical JSON配列 `[source_id, toolName]`)。同名でもsourceが違えば別ID。同sourceの再同期・説明更新でtool_idは変わらない。
- revision_id = `mcpr_` + SHA256(canonical JSON配列 `[tool_id, endpoint_hash, normalized_descriptor]`)。descriptorには完全な説明、input/output schema、annotations、明示的管理metadataを含める。search用4KiBへ切る前の本文でhashを作る。
- JSON object keyは再帰的にソートしarray順は維持する。未知の表示metadataは保存対象を明示し、hashへ含めるものを固定する。同じ内容のkey順変更でrevisionを増やさない。
- bindingは `{kind:"mcp_http",sourceId,toolName,endpointHash}`。dispatchはDBで選んだrevisionとsource設定を照合する。LLM引数のURLやkindを利用しない。
- MCPはremote toolの過去revision実行を保証しない。SAAAが固定できるのは観測したdescriptorと接続先。観測後にremote実装が変わる競合を完全に防げるとは記載しない。既知の更新後は旧参照を拒否する。

同期手順は必ず次の順序にする:

1. sourceごとのmutexで同時同期を1件にする。設定generationを捕捉しinitialize済み接続でtools/listの全pageを取得する。
2. メモリにstagingする。最大10,000件/source、100,000件/profile、2,000page、1page最大4MiB、全page合計64MiB、取得全体60秒。cursor循環・重複toolName・不正schema・上限超過は同期全体失敗。空配列でcursorが進むpageは許可する。
3. schemaは既存validatorが扱える範囲で検証する。未対応schemaは勝手に `{}` へ置換せず、その同期を失敗にする。toolの完全description最大128KiB、input/output schema各8KiB、describeの既存16KiB envelopeに収まらない定義も同期失敗とする。
4. 正規化してrevisionを決定し、usageページをUTF-8境界で各8KiB以下に分割する。description全文はusageに保存。必須引数はinputSchema.requiredから取得する。MCPから不明なoperation/objectは推測で断定せずunknown扱いとし、LLMによる一括metadata生成は行わない。annotationsのreadOnly等は認可根拠にしない。effectは管理metadataがなければunknown。
5. 1 transactionでsource generationを再確認し、追加・変更・再出現tool、usage、FTSとcurrent_revisionを公開する。消えたtoolはdisabledにし履歴を削除しない。FTS・embedding検索はcurrentかつenabledのrevisionだけが対象になるよう制限する。
6. 公開内容の変更時だけcatalog_epochを1回増やす。同一内容再同期はepoch不変。説明がA→B→Aへ戻る場合も既存revision Aへポインタを戻す。既存revisionを見つけて早期returnしてはいけない。
7. 成功時刻を更新し、設定由来grantを別transactionで差分反映する。その後、新revisionだけembeddingを生成する。生成失敗はBM25へdegradedとして縮退し、後続同期で欠損のみ再試行する。旧revisionベクトルを新revisionへ付け替えない。

通信・検証・DB途中失敗では前回の完全snapshotを残す。source障害はlast_errorに記録する。失敗を空一覧と解釈してtoolを消さない。成功同期後300秒以上経過したsourceは検索適格性・describe・新規invokeで拒否する。残存candidateRefも拒否するため、epochだけに依存せず鮮度を再確認する。再起動後は一度同期成功するまでremote invokeを許可しない。

## 5. HTTP・接続管理

transportの互換条件は[公式Streamable HTTP仕様](https://modelcontextprotocol.io/specification/2025-06-18/basic/transports)を参照する。クライアントはPOSTへのJSON応答とSSE応答の両方を処理する。JSONだけを実装して互換完了としない。

実装する接続状態は `Disabled → Initializing → Ready → Reconnecting / Unavailable → Closing`。sessionはprofile/source/config_generation単位で管理し、別sourceへ共有しない。initializeのprotocolVersion不一致・tools capability欠落を拒否し、initialized通知完了前にlist/callしない。返されたsessionヘッダーとversionヘッダーを後続requestへ付ける。

HTTP・SSEの詳細判断:

- POSTのAcceptは `application/json, text/event-stream`。通知は202空bodyを処理。JSON-RPC request IDはcall_id等の文字列で発行し、応答では値と型を一致確認する。
- SSEはライブラリ利用を優先する。独自実装ならchunk境界、複数data行、CRLF、comment、UTF-8分断を試験する。イベント上限4MiB、call応答全体4MiB、余分な通知は個数・bytesを制限する。無限progressでdeadlineを延長しない。
- 任意GET SSEを開きlist_changedを監視する。405は正常な未提供として扱う。通知は500ms debounce、dirty flagで同期中に来た変更も次回へ持ち越す。通知がなくても60秒ごとに同期する。source単位で失敗backoffは1/2/4/8/16/30秒上限とする。
- connect timeout5秒、initialize10秒、list取得全体60秒。invoke deadlineは既存BackendRequest.timeoutを使用。接続待ち時間もdeadlineに含む。source内call最大4、profile全体16、待機queue最大64。満杯は送信前busy。
- session失効404では次の処理用に再initializeする。同時再接続は1件にまとめる。進行中tools/callは自動再送しない。GET再接続とPOST再実行を混同しない。
- shutdownは新規受付停止→進行中callに取消要求→最大5秒待機→残件をunknownに終端→session DELETE best effort→task join。DELETEの405は異常扱いしない。
- transport切断と実行取消は別事象。副作用実行の有無が判明しないものをfailed/cancelledで断定しない。

## 6. 実行結果・取消・継続取得

BackendRouterはDB由来bindingのkindを検証し、llangは既存LlangBackend、mcp_httpはMcpBackendへ渡す。未知kind、不一致source/toolName、変更済endpointは送信前拒否。既存L-Lang binding互換は型として明示し、欠落kindを任意のremoteへfallbackしない。本番経路からFixtureBackendを呼ばない。

認可・source有効性・鮮度・revision・schema・rule/acl/catalog epochを送信直前に再検査する。DB lockを通信中保持しない。送信許可判定とdispatch開始を同じsource gateで直列化し、設定停止が先なら送信しない。すでに送ったrequestの副作用は取り消せるとは保証しない。

| 観測した事象 | technical_status / 振る舞い |
| --- | --- |
| 送信前の入力不正・権限不足・source不適格 | backend call数0。既存gateway error規約を使用 |
| 有効なtools/call結果、isErrorなし/false | succeeded。満足度はunknownのまま |
| 有効なtools/call結果、isError:true | failed / remote-tool-error。内容はbounded resultとして残す |
| 対象IDのJSON-RPC error | failed / remote-rpc-error。生の内部情報は通常ログへ出さない |
| 送信後timeout・切断・壊れた応答・上限超過で結果未取得 | unknown / remote-outcome-unknown。自動retry不可 |
| 送信前取消 | cancelled、HTTP tools/callは0回 |
| 送信後取消 | notifications/cancelledをbest effortで送る。確定結果がなければunknown。通知送信成功をremote停止成功とみなさない |
| 完了済み結果を取得した後の取消 | 先に確定した結果を保つ。二重終端しない |

呼出元futureのabortでも管理taskが取消とDB終端まで所有する。permitはそのtaskが保持し必ず解放する。再起動reconcileは外部callを「中断したがremote結果不明」と識別できるコードで記録し、再実行しない。既存L-Langの実際に停止を確認できるcancelled契約を弱めない。

MCP結果は[公式tools仕様](https://modelcontextprotocol.io/specification/2025-06-18/server/tools)のcontent・structuredContent・isErrorを区別して処理する。textを二重JSON decodeしない。image/audio/resourceはtypeを保って保存し、URLを自動取得しない。unsupportedなcontentは黙って成功textへ変換しない。outputSchemaがある場合は対応するstructuredContentを検証し、不正結果を通常成功として提示しない。

大結果は今回実装する。16KiB以上の正常結果を単純failedに落とさない:

1. 既存の小結果形式は維持する。正規化結果が16KiB超〜1MiBならSQLiteへ保存し、invokeはstatus・invocationId・resultRef・byteCount・pageCountを返す。reply envelopeを含め16KiB以内にする。
2. `tools_describe` に `{resultRef, page}` 分岐を追加する。既存候補説明分岐と相互排他で、JSON schemaでもoneOfにする。ツール数は3件のまま。
3. canonical JSONのUTF-8テキストを最大8KiBのpageへ分割し、`encoding:"json-text", text, page, pageCount` で返す。全pageの連結で元JSONと一致する。binary contentはJSON内のbase64のまま。previewをSystemContextへ入れない。
4. resultRefは推測困難なID、TTL10分、principalとrun/scopeにbind。取得時に現在ACLを再確認する。rule更新やremote revision更新で既に取得済み結果を別の結果へ差し替えない。所有者違い・期限切れ・認可取消は取得拒否。
5. 保存量はprofile32MiB、1run最大64件。期限切れを除去しても不足なら新規保存を拒否する。内容を保存できなくても、確定済remote成功はsucceededのまま `resultAvailability:"unavailable", error_code:"result-storage-limit"` とする。大きさが1MiB超でも同様にresult-size-limit。remote結果を最後まで検証できなかった場合はunknownで区別する。
6. raw resultは通常ログに出さずDBの既存保護方針に従う。終了runと期限切れのcleanupを起動時・保存時・定期処理で実行する。secretヘッダーは結果に混ぜない。

## 7. 検索と訂正記憶を壊さないための変更

ランキングアルゴリズム、既存scope条件、unknownの意味は変更しない。各候補・describeにsourceIdと表示用sourceLabelを付ける。同じsearchという名前でもどの接続先か区別できるようにする。

訂正対象は安定tool_idで保存する。抽出器にはdecision内のtool_id/sourceId/toolNameを少数候補として渡す。LLMが返したIDをそのまま信用せず、対象decision・principal・権限に照合する。名前だけの場合は対象decision内で一意な候補に限る。追加のpreferred候補は認可済み集合で一意なsource付き指定の場合だけ解決する。複数sourceに同名があり特定できなければambiguousとし、LIMIT 1で選ばない。

tool選択への訂正はrevision更新後も同tool_idの一致scenarioへ適用する。revision固有の不具合指摘は新revisionへ自動継承しない。消えたtoolへのruleは保存するが候補へ復活させない。再出現時も現在のACLを先に検査する。source URL変更では古い訂正ruleを自動適用せず、管理上の再確認が済むまで保留扱いにする。

無反応・HTTP200・isError:falseは満足度を増やさない。今回の変更で新しい学習器、全ユーザー共通penalty、推測positive labelを導入しない。

## 8. 実装カードと合格条件

各カードは実装→指定試験→差分確認を終えてから次へ進む。既存ファイル名は2章に対応し、新規ファイル名は下表を用いる。巨大な単一mcp.rsを作らない。

| ID / 依存 | 実装箇所・作業 | そのカードの合格条件 |
| --- | --- | --- |
| T00 / なし | HEAD・status・AGENTS・既存検証を記録。既存検索/訂正/provider/generated試験を実行 | 元からの失敗と今回の失敗を区別できる。実装済み報告だけで通過しない |
| T01 / T00 | `mcp/config.rs`、contracts。設定parse・URL・secret参照・grant宣言の型 | 省略時互換、重複ID、未知field、悪いURL、missing tokenの表駆動試験 |
| T02 / T01 | `mcp/repository.rs`、schema、persistence migration | v23実DB fixtureからupgrade。既存rule/usage/grant/invocationの値不変、再open可能、FK正常 |
| T03 / T02 | `mcp/descriptors.rs`、catalog。sourceとrevision登録の分離、安定ID | 同名別source、key順差、説明末尾更新、A→B→A、schema/endpoint変更を検証 |
| T04 / T01 | `mcp/transport.rs`、`mcp/session.rs`。JSON-RPC/HTTP/SSE接続 | 実HTTPテストサーバーでJSON/SSE、init順、ID照合、session有無/404/405、上限を検証 |
| T05 / T03,T04 | `mcp/sync.rs`。全page取得・検証・atomic publish | 途中失敗で前snapshot完全維持、no-opでepoch不変、変更batchで+1、消失/再出現 |
| T06 / T05 | `mcp/manager.rs`、mod起動配線。poll・通知・grant差分・停止 | sync多重なし、通知取りこぼしなし、設定削除で送信停止、import単独では権限なし |
| T07 / T06 | `backends/router.rs`、`backends/mcp.rs`、service invoke | 実L-Langと実HTTP backendを同gateway経由で実行し混線なし。unknown/abort/permit回収 |
| T08 / T07 | `mcp/results.rs`、gateway、contracts。継続取得 | 全page復元一致、16KiB envelope、scope/TTL/ACL、保存不可でも技術結果を改変しない |
| T09 / T05,T07 | repository eligible検索・embedding差分・source freshness | 旧revision/disabled/stale/未認可の候補数0、欠損embedding縮退、再同期で回復 |
| T10 / T09 | feedback、provider_extraction、候補response | 同名別sourceで誤訂正0、自然な訂正→次回順位を既存会話経路で確認 |
| T11 / T08,T10 | `tests/mcp_*.rs`、実モデル評価と実HTTP負荷測定 | 下記A01〜A15を満たす。mockとrealを区別して記録 |
| R1 / T11 | 初回実装snapshotを固定し全差分を自己レビュー | 各指摘に再現条件・影響・根拠行・修正方針を記録。mdを実装品質の根拠にしない |
| R2 / R1 | 修正・該当試験・最終回帰・検証報告 | 重大問題の未解決なし。未達項目は完了扱いにせず列挙 |

T04のHTTPサーバーfixtureは同期・実行テストで共用するが、assertionを本番実装のhelperだけで作らない。期待するJSON・DB状態・呼出回数を試験側で定義する。実ネットワークの外部サービスや有料APIを単体試験の前提にしない。

## 9. 受入試験

| ID | 再現方法と期待結果 |
| --- | --- |
| A01 | 2source合計1,500toolsを各100件pageで同期。常時LLM定義は3件、SystemContextに全説明なし、検索最大8件 |
| A02 | 同期page3で500・不正JSON・重複名・cursor循環をそれぞれ注入。旧snapshot/epochが不変 |
| A03 | 同内容再同期、説明末尾のみ変更、A→B→A、tool消失→再出現。IDとepochが4章どおり |
| A04 | 未認可・別project・別principal・stale referenceでsearch/describe/invoke拒否、HTTP call数0 |
| A05 | 同名toolをL-Langと2MCPに配置。source付き実行で指定先のみ1回呼ぶ。曖昧訂正でrule増加0 |
| A06 | JSONとchunk分割SSEのtools/call双方。通知やprogressを挟んでも対象IDの結果だけ返す |
| A07 | serverが副作用カウンタを増やした後切断。unknown、retryable false相当の契約、カウンタ1。再接続でも再送なし |
| A08 | 送信前取消はcall0。送信後取消・caller abortは終端記録とpermit回収。remote停止を断定しない |
| A09 | sync中設定変更、disable直後dispatch、list_changed連発、shutdown中callの競合をbarrierで再現。sleep頼みで順序を作らない |
| A10 | 100KiB日本語結果をページ復元。別run拒否、TTL経過拒否、ACL撤回拒否、容量上限で副作用再実行なし |
| A11 | ユーザー自然発言で外部toolの訂正。該当scenarioだけ順位変更、別project/operationは不変、再起動後も維持 |
| A12 | 無反応100call、isError:true/false、timeoutを混在。技術状態は変わるが満足度positive件数は増えない |
| A13 | real E5/BGEで外部descriptorを検索。既存held-out評価の基準を維持。1500/10000件のp50/p95・使用memory・prompt bytesを記録 |
| A14 | v23→新version→再起動で既存L-Lang呼出、訂正、World Modelの既存試験が通る。FixtureBackendの本番配線なし |
| A15 | 401/403、redirect、missing env、SSE過大、unsupported server request。secretのログ露出なし、sampling等の実行なし |

精度測定には、実際に同期したdescriptorを使った外部MCP用のheld-out scenarioを最低30件追加する。同名衝突・no_match・訂正を含める。評価用正解をdescriptionへ追記して精度を上げない。fixture輸送試験だけを実ML評価と呼ばない。実モデル未配備ならT11未完了として報告する。

## 10. 自己レビューと提出

最低限、次の経路をファイルをまたいで追跡する:

1. 設定→source→initialize→全page→公開transaction→grant→検索→describe→認可再検査→HTTP1回→終端。
2. ユーザー訂正→decision→source/tool一意性→scope→rule→revision更新→次回順位。
3. 大結果→保存→resultRef→別run/ACL変更/期限切れ→拒否→cleanup。
4. 送信後切断/abort→unknown→再接続→再送されないこと→permit解放。

開始時と最終時に実行する基本コマンド（repo root）:

```sh
cargo test --manifest-path src-tauri/Cargo.toml tool_selection
cargo test --manifest-path src-tauri/Cargo.toml generated_capabilities
cargo test --manifest-path src-tauri/Cargo.toml providers
cargo fmt --check --manifest-path src-tauri/Cargo.toml
cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings
bun run size:check
```

最終は `bun run check` も実行する。新規moduleは既存size登録規約に従う。実ML・負荷試験は個別コマンドを用意し、manifest/hash、dataset、seed、hardware、cold/warm条件と結果を残す。都合の悪い試験の削除、上限の緩和、ignored化で合格させない。

成果物は実装・試験・ `spec/docs/verification/tool-selection-d4.md` 。報告には開始/初回/修正後のsnapshot、実施カード、変更ファイル、migration結果、試験件数、HTTP/MLの実測、自己レビュー指摘と修正、未達・制限を記す。D5のHTTPサーバー公開やD6学習まで完了したとは記載しない。

## 11. 担当AIへ渡す依頼文

> 本書のD4をT00からR2まで実装してください。現在のD0〜D3の3入口・SQLite台帳・条件付き訂正記憶へ、外部MCP Streamable HTTPを接続します。カードの入出力と受入試験を守り、同名source衝突、atomic同期、importとgrantの分離、送信後の不確定結果、大結果の継続取得を省略しないでください。初回実装のsnapshotを残して自分で全差分をレビューし、問題を再現・修正してから最終検証してください。既存L-Lang実行基盤とWorld Modelを保ち、別タスクへのメッセージ送信、D5/D6への拡張は行わないでください。未達を完了として報告しないでください。
