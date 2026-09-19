# SAAA × L-Lang — M2A 会話ツール接続の実装計画

作成日: 2026-09-19  
状態: 実装指示。今回はこの文書の作成のみ。  
上位文書: [初期実装計画](saaa-llang-dynamic-capability-initial-plan.md)、[M0/M1詳細手順](saaa-llang-dynamic-capability-implementation-guide.md)

## 1. 今回の到達点

既存のL-Lang packageを検証・有効化した後、管理者が明示的に公開した能力だけを、SAAAの会話でLLMがツールとして呼び出せるようにする。LLMへ渡した定義と実行するrevisionを一致させ、更新・停止・会話取消の競合でも別の能力を実行しない。

M2を二分する。今回の実装は **M2A＝共通公開契約とChat Completions経路への接続**。次回のM2Bで同じサービスに認証付きローカルMCPを接続する。今回はMCP listener、LLMによる生成、管理UI、TypeScript inspection、world clock、新しいABI、effect、他providerへの独自ブリッジを作らない。M2全体の完成と報告しない。

完成例: fixture Aをimport → verify → activate → 公開設定へ登録 → 会話リクエストにboolean入力ツールが出現 → LLMのtool callを既存CapabilityServiceで実行 → boolean結果を同じ会話へ返す。公開されていない能力はactiveでも出現しない。

## 2. 作業前提と現状の区別

2026-09-19の作業ツリーにはM1コードと追加修正があり、未コミット・未追跡ファイルを含む。shutdown制御、runtime再検証、追加回帰テストがあるが、この計画書の作成時点では修正の合格を認定していない。工程G0で判定する。

以下の新規型・設定・ファイル名は今回の設計指定であり、実装済みという意味ではない。既存APIと異なる場合は薄い変換を追加し、検証や状態管理をprovider側に複製しない。

| 読む場所（repository rootからの相対パス） | 確認事項 |
| --- | --- |
| `src-tauri/src/generated_capabilities/service.rs` | resolve_active、InvokeRequest、invoke、shutdown、実行所有権 |
| `src-tauri/src/generated_capabilities/{contracts,errors,repository,lifecycle}.rs` | boolean subset、状態、epoch、DBの境界 |
| `src-tauri/src/generated_capabilities/host/` | runtime検証、プロセス終了、Cancellation |
| `src-tauri/src/generated_capabilities/tests/` | 既存テストが実際に再現する条件 |
| `src-tauri/src/providers/stream/dispatch.rs` | 定義生成と実行振り分け、末尾のrecall fallback |
| `src-tauri/src/providers/chat_completions/mod.rs` | リクエストごとのtools、バッチ事前検証、再試行制御 |
| `src-tauri/src/providers/chat_completions/voice_progress.rs` | execute_agent_toolへの呼出経路 |
| `src-tauri/src/providers/stream/attempt.rs`、`src-tauri/src/app_state.rs` | AppState共有、RunCancellation |
| `src-tauri/src/lib.rs`、`src-tauri/src/test_support.rs` | 起動、終了、テスト用AppState |

作業順は `G0 → A1 → A2 → A3 → A4 → A5 → R1 → R2`。G0で失敗したら必要なM1修正と再検証を先に行う。工程を飛ばして「残りは今後の課題」としない。

## 3. G0: 開始時の証拠とM1の入口検査

### G0-1 開始状態を保存する

HEAD、git status、追跡ファイルのdiff、未追跡コードの一覧を記録する。今回変更するファイルの開始時コピーとSHA-256一覧をrepository外の一時ディレクトリに保存する。既存の未コミット実装を今回の変更と区別するためである。秘密情報・ユーザーデータ・target・node_modulesはコピーしない。reset、stash、clean、無断commitは禁止。

### G0-2 次の再現条件を確認する

既存テスト名の存在だけでは合格しない。期待値が下表に一致することを読み、足りなければテストを追加する。任意sleepだけに依存せず、開始通知やbarrierで競合点を固定する。

| ID | 操作 | 合格条件 |
| --- | --- | --- |
| G01 | サービス構築後に信頼runtimeの実ファイルを変更し実行 | 改変したruntimeを起動せず拒否 |
| G02 | 同じDBでruntime digestの異なる正規設定へ切替 | 古い検証結果でactivate/invokeできない。DBのdigestを書き換えるだけの試験では不可 |
| G03 | verify後に管理下package実ファイルを変更してactivate | 改変を拒否しactiveにしない |
| G04 | 実行枠を占有して別importを開始 | inspectも共通上限に従い、追加子プロセスが走らない |
| G05 | invoke実行開始後にshutdown | 受付停止、子プロセス停止・回収、call終端、枠解放。終了後のimport/verify/invokeを拒否 |
| G06 | verify開始後、完了前にsuspend | active化しない。checkがrunningのまま残らない |
| G07 | 実行futureの所有者をdrop/abort | 子プロセスと実行登録が残らず、DBが所定の終端状態になる |

G05/G07は非同期executorを同期sleepで塞いで完了待ちにする設計にも注意する。終了APIをsyncのままにする場合も、実際のアプリ終了呼出経路で回収が進むことを試験する。単にtimeout後にfalseを返して正常終了扱いにしない。

`generated_capabilities`と`wasm_host_poc`の既存テストを実行する。失敗は開始時からの失敗か修正由来かを分ける。G01〜G07に関わる失敗を残してA1へ進まない。修正は独立した差分として記録する。

## 4. 今回固定する公開契約

### 4.1 明示的な公開設定

新規環境変数 `SAAA_GENERATED_TOOLS_CONFIG` はJSON設定ファイルの絶対パスとする。既存runtime設定とは別に読み込む。起動時に一度だけ読み、今回hot reloadは行わない。

```json
{
  "formatVersion": 1,
  "enabled": true,
  "capabilityIds": ["実際のcatalogにあるcapability ID"]
}
```

- 未設定またはenabled=falseなら公開なし。既存会話機能は動作する。
- 未知キー、未知version、重複ID、空ID、9件以上、64 KiB超、読めないファイル、不正JSONは公開機能だけを無効にして安全な診断を残す。黙って先頭8件に切らない。
- IDはmetadata.idやrevision IDでなくcatalogのcapability ID。生成package自身が公開設定を書き換える機能を追加しない。
- enabled=trueかつ空配列は有効な「公開ゼロ件」。最大8件。
- 設定済みIDが未存在・inactive・suspendedなら、そのIDはofferから除外する。DB障害・契約破損・名前衝突・サイズ超過なら当該リクエストの生成ツール全体を公開しない。既存ツールは保持する。
- 同一AppStateの既存Arc<CapabilityService>を使う。二つ目のservice、writer、catalogを作らない。

設定オブジェクトはAppStateに保持し、test_supportの全初期化箇所も更新する。公開変更には再起動が必要と明記する。設定ファイルの本文や実パスをLLMに渡さない。

### 4.2 ツール名、入力、説明

名前は `gc_` + revision UUIDのハイフンなし小文字32桁。例示用metadata名から生成しない。不正なUUIDを短縮hash等で救済しない。同一snapshotに名前衝突があれば公開を失敗させる。

入力schemaは検証済みWasmContractから構築する。object、boolean propertyのみ、全項目required、additionalProperties=false。M1の1〜8項目制限を維持する。任意の候補JSON Schemaをそのままproviderへコピーしない。

descriptionはホスト管理の定型文とする。「検証済みboolean判定。列挙されたすべてのboolean入力を指定するとboolean結果を返す」という意味を明示し、フィールド名はschemaに載せる。今回は候補由来の自由文説明・使用例をsystem promptに挿入しない。業務的な使い分けを説明する充実したメタデータは別工程にする。

生成ツール定義配列をcompact JSONへserializeしたUTF-8バイト数を32 KiB以下にする。既存ツールはこの生成ツール専用上限に含めない。固定の並び順はツール名の昇順。

### 4.3 不変のoffer snapshot

共通モジュールに `GeneratedToolSnapshot` を追加する。最低限、定義配列と `tool_name → ResolvedCapability` の対応を所有する。ResolvedCapabilityのrevision_id、package_hash、contract_hash、catalog_epochを保持する。

公開対象の解決は一つのcatalog読取境界で行い、複数IDの途中で更新を挟んだ混合snapshotを作らない。SQLロックを保持してhostを起動してはいけない。

snapshotの寿命は **一回のprovider HTTPリクエストと、そのレスポンスが返したtool callバッチの実行完了まで**。会話全体・run全体・グローバルcacheに固定しない。次のLLMリクエストでは再生成する。

実行時はsnapshot内のResolvedCapabilityをInvokeRequestへ渡す。tool名から現在revisionを再解決して置き換えない。serviceの受付時に現行epoch・状態・hashを再検査する。offer後に更新・停止があれば古いofferの呼出しを拒否する。受付済み処理は既存M1の方針に従い、途中で新revisionへ切り替えない。

### 4.4 実行結果と識別子

provider由来call.idはそのままDB主キーにしない。内部call IDは毎回ホストでUUIDを発行する。invokeのoriginは `conversation`。providerのtool_call_idは会話応答の対応付けに維持する。

成功時のtool contentは次のJSON。value=falseも成功である。

```json
{"ok":true,"callId":"host UUID","revisionId":"revision UUID","value":false}
```

失敗は `{"ok":false,"error":{"code":"安定コード","message":"安全な説明"}}`。既存CapabilityErrorCodeを明示的にマッピングし、invalid-input、not-active/stale、busy、cancelled、timeout、unavailable、integrity系を区別する。内部path、stderr、設定、SQL、stack traceを返さない。対応表をコードとテストに置く。

入力は16 KiB以下のJSON object。欠落・余剰・非boolean・不正JSONをhost起動前に拒否する。service側検証も維持する。未知のgc_名は失敗させ、recall等のfallbackに流さない。

## 5. 工程カード

### A1: 公開設定とsnapshot（providerを変更する前）

推奨ファイルは `generated_capabilities/publication.rs` とそのテスト。設定parse、上限、名前、schema、snapshot生成を実装する。catalogの一括読取はservice/repositoryの責務。providerのJSON形式への変換はadapterへ分け、将来のMCPがOpenAI形式に依存しない共通定義にする。

完了条件: P01〜P06合格。disabledでhostやDBを呼ばない。説明のために新しいDBテーブルを作らない。

### A2: 共通実行adapter

推奨ファイルは `generated_capabilities/tools.rs`。snapshotからの名前lookup、入力parse、内部ID生成、service.invoke、安全な結果変換を実装する。直接Hostを呼ばない。MCP/会話の実行ロジックを将来分岐コピーしなくて済むよう、originと取消を引数にできる境界を設ける。MCPの仮実装は作らない。

完了条件: E01〜E06合格。active再解決によるrevision差替えがない。

### A3: Chat Completionsへの配線

`available_agent_tools`の結果と生成snapshotを一体として当該リクエストに保持する。既存API変更が広がる場合は新しいoffer構造体または専用生成処理を追加してよいが、definitionsとsnapshotの別々の再生成は禁止。

`chat_completions/mod.rs`で既存toolsに生成定義を追加し、受信バッチ全体のtool_was_offered検証を維持する。未知名を一つでも含むバッチは、どのツールも実行する前に拒否する。既存のmark_started、最大32call、parallel_tool_calls=falseを維持する。生成ツールの追加offerは既存coding/UIと同様calls_this_attempt < 12とする。既にofferした同一バッチは既存の32件上限で扱う。

生成ツールは対応snapshotを伴ってadapterへ渡す。voice_progress/dispatchを経由する場合は必要な引数を明示的に伝播する。既存ツールの音声進捗処理を壊さない。生成ツールの進捗音声は今回は追加しない。

options.tools=false、JsonProbe、output_persistenceなしでは生成ツールを公開しない。別providerは今回未対応で、使えるように見せる定義だけを追加しない。

完了条件: C01〜C05合格。関数単体でなく実際のHTTP request bodyとtool結果メッセージを試験する。

### A4: 取消・timeout・終了

RunCancellationとhost Cancellationを接続する。invoke futureと会話取消をselectするだけでfutureを捨てて完了にしてはいけない。取消を通知し、所有する処理の終了・DB終端・permit解放まで責任を持つ。外側futureのdrop/abortにも対応する。非同期後処理が必要ならservice所有のtaskと終了時drainを使い、孤立したfire-and-forget taskを作らない。

実行timeoutは既存serviceの上限を使い、会話側残り時間を超えるなら短い方に制限する。期限切れ時も取消・回収を行う。SQLトランザクション中にawaitしない。RunCancellation通知はpoll間の通知取りこぼしも防ぐ。

完了条件: X01〜X04合格。RunCancellationを既にcancelしただけの試験で実行中取消の代用をしない。

### A5: 一連の利用と後方互換を検証

テスト用HTTP providerが一回目に実在するgc_名でtool callを返し、二回目にtool結果を受け取って通常回答を返すfixtureを作る。既存のmock server方式を優先する。ネット接続・有料LLM・本番秘密情報は不要。

fixture A/Bと既存の正規runtimeを使う統合テストを最低一本含める。fake hostだけで全件を完了させない。設定ファイルの例、import/verify/activateに使う既存テスト支援経路、実行コマンドを検証報告に記す。会話から管理操作を公開しない。

## 6. 必須試験表

| ID | 条件と観測する結果 |
| --- | --- |
| P01 | 設定なし/disabled/空配列で生成定義ゼロ、既存tools維持 |
| P02 | 未知キー・version・重複・9件・64 KiB超で明示的に無効化 |
| P03 | allowlist外activeは非公開、allowlist内inactiveも非公開 |
| P04 | schemaは全boolean必須、余剰禁止。名前はrevisionに一意 |
| P05 | 32 KiB境界の前後を試験。超過で全生成定義を拒否し黙って切らない |
| P06 | offer作成と更新の競合でもsnapshotが一つの読取状態に対応 |
| E01 | Aのtrue/false両方を実行しfalseもok=true |
| E02 | JSON不正・array・欠落・余剰・非boolean・16 KiB超でhost起動ゼロ |
| E03 | 未offer名・偽造gc_名を拒否しrecall実行ゼロ |
| E04 | 同じprovider call.idを別runで使っても内部call IDは別 |
| E05 | offer A後にBをactivate。A呼出し拒否、Bへ自動差替えなし。次offerはB |
| E06 | offer後suspend・runtime/package改変で実行拒否。内部情報の漏出なし |
| C01 | mock providerへの実HTTP bodyに生成定義、次requestに正しいtool_call_idと結果 |
| C02 | offered+unknownの混合バッチを全体拒否し最初のcallも未実行 |
| C03 | tools=false/JsonProbe/永続stateなしで非公開、call数上限を維持 |
| C04 | disabled時に既存recall/coding/UI/voiceの該当回帰テスト合格 |
| C05 | 実行後のprovider失敗で同じ副作用を自動再試行しない既存制御を維持 |
| X01 | 起動前取消でhost起動ゼロ。起動後取消で停止・回収・DB終端 |
| X02 | provider処理futureをabortしても子プロセス・running call・登録・permitが残らない |
| X03 | 実行枠競合でbusy/既存規定の拒否。capacity上限を越えない |
| X04 | timeoutとshutdownでも回収完了。次回起動のrecoveryに通常取消を丸投げしない |

P05の境界入力が実contract制限では作れない場合はサイズ計測関数の境界テストと、実contractの上限内確認を分ける。制限を緩めて大きなfixtureを成立させない。

## 7. R1: 実装者自身による差分コードレビュー

最初の実装と必要試験が完了した時点で、**修正前の自己レビュー対象snapshot**をrepository外へ保存する。開始時との差分と新規コード全文を保存し、その後の修正と分ける。これにより初回実装力と自己修正力を区別できる。

レビュー対象はRust、設定処理、SQL変更があればSQL、テスト、fixture、依存変更。Markdownの出来栄えを実装品質の証拠にしない。

次の順序でコードを読み直す。テストがgreenであることだけを理由に省略しない。

1. 今回の開始時からの差分と新規ファイル全文を読む。git diffだけでは未追跡ファイルが落ちるため一覧と突き合わせる。
2. HEADからの全コード差分も読み、M1との接続破壊・既存機能の削除を確認する。ファイル移動は移動先、呼出元、同等動作まで追う。
3. 一つのcallについて設定 → offer → HTTP → parser → dispatch → service → host → DB終端 → tool応答を追跡する。対応するfile:lineを記録する。
4. offer後更新、受付直前停止、host起動直後取消、timeout、DB終端書込失敗、外側futureのdropの各位置で、残る子プロセス・DB状態・実行枠・所有者を確認する。
5. 実際のruntime差替え、実ファイル改変、実行中取消がテストされているか確認する。DB値変更・起動前取消だけなら不足を修正する。
6. host validationの迂回、snapshotの再解決、recall fallback、秘密情報漏出、上限の黙った切詰め、unsafeな再試行を探す。

各発見は `重要度 / file:line / 発火条件 / 期待と実際 / 根本原因 / 再現テスト / 修正 / 再検証` で記録する。問題がなければ、調べた経路と実施した反例試験を記録する。指摘件数を稼がない。

不具合を発見したら、可能な限り修正前に失敗する再現テストを作り、修正後に合格させる。仕様やテスト期待値を実装の誤動作に合わせない。テスト困難なら理由と代替観測を明記し、「検証済み」にしない。

## 8. R2: 修正、最終検証、提出

自己レビュー指摘を修正し、影響する試験を再実行する。未解決のP0/P1、今回の契約違反、再現する回収漏れがあれば完了にしない。無関係な既存不具合は勝手に大改修せず分離して記録する。

最低限、次を実行する。providerテストの実際のmodule名は探索して追加実行し、フィルタが0件だった場合は成功に数えない。

```sh
cargo test --locked --manifest-path src-tauri/Cargo.toml --lib generated_capabilities
cargo test --locked --manifest-path src-tauri/Cargo.toml --lib wasm_host_poc
cargo test --locked --manifest-path src-tauri/Cargo.toml --lib providers
cargo fmt --check --manifest-path src-tauri/Cargo.toml
cargo clippy --locked --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings
bun run size:check
```

最後にroot AGENTS.md等で要求される検証と、`bun run check`を実行する。既存失敗・依存不足は実行結果と開始時の状況を明示する。サイズ上限やlintを通すためだけにbaselineや閾値を引き上げない。大きくなるservice/providerファイルから今回の責務を専用moduleへ分ける。

提出物は実装コード、試験、`spec/docs/verification/llang-dynamic-capability-m2a.md`。報告には次を含める。

- M2Aとして実際にできることと未対応provider。
- G01〜G07、P/E/C/X各IDと対応テスト名・結果。
- 開始時、自己レビュー前、修正後のsnapshot場所と差分の区別。
- 自己レビューで見つけた問題と修正結果。ゼロなら確認した具体的経路。
- 実行コマンド、実際の件数、失敗、未実施理由。
- M2Bへの引継ぎとして共通snapshot/adapter APIと、MCP未実装である旨。

自分で点数を付けて品質を保証しない。提出証拠から第三者が初回実装と修正後を評価できるようにする。

## 9. DeepSeekへ渡す実行指示

> `spec/docs/saaa-llang-dynamic-capability-m2a-plan.md`に従い、G0からR2まで実装してください。対象はM2Aです。最初に既存の未コミット・未追跡コードを含む開始状態を保存し、M1の入口検査を行ってください。会話ツール接続は既存のCapabilityServiceを使い、offer snapshotと取消・終了の所有権を維持してください。実装後に修正前snapshotを保存し、自分で全差分と新規コードをレビューしてください。見つけた問題は再現・修正・再検証まで行ってください。Markdownの充実やテスト件数を実装品質の代用にしないでください。MCP、UI、LLM生成、ABI拡張には進まず、実施した検証と未解決事項を正確に報告してください。
