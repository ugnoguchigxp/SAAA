# SAAA × L-Lang 動的拡張 — 詳細実装手順・契約・試験仕様

作成日: 2026-09-19  
状態: M0/M1の初期実装仕様・記録 / 未実装M3/M4は統合計画へ移管
上位文書: [初期実装計画](saaa-llang-dynamic-capability-initial-plan.md)

## 0. 担当AIが最初に読むこと

最初の依頼範囲は**M0とM1**である。M2〜M4は後続の依頼で実装する。この文書にコード風の型、ファイル名、設定値がある場合、「既存」と明記したもの以外は今回採用する設計であり、現在の実装が存在するという意味ではない。

作業順は `事前確認 → T00 → T01 → T02 → T10 → T11 → T12 → T13 → T14 → T15 → 完了報告`。T03はinspectionの依存確認として実施するが、未達でもT02まで合格していればM1の独立した作業を進めてよい。先の工程のための空実装や成功を返すstubを作らない。

変更対象はSAAA。L-Langのソースは読み取りと固定版からの成果物生成に使う。L-Langへの修正が必要と判明したら、必要な契約・再現入力・期待出力を依存事項として文書化する。別タスクへの連絡、別repositoryの既存変更の修正・commitは行わない。

### 作業前の確認

1. `git status --short`と両repositoryのHEADを記録する。ユーザーの変更を取り消さない。計画作成時にはSAAAにコンセプト文書・計画書・`.commandcode/`の未追跡項目があった。作業対象外の項目には触れない。
2. プロジェクトの指示を確認する。`initial_instructions`は同じ作業セッションで未実行なら一度だけ実行する。
3. 本書の「既存ファイル」を開き、前回の調査と差がないか確認する。ファイル移動だけなら新しい位置を記録して続ける。契約が変わっていればT01の比較を更新してから進む。
4. 開発用の一時ディレクトリと一時DBだけで検証する。日常利用中のSAAA DBに試験用candidateを入れない。
5. 既存PoC対象試験と`bun run check:local`を一度実行しbaselineを保存する。環境不足による失敗、既存失敗、今回の変更による失敗を分ける。成功件数は実際の出力から記録する。

### 変更範囲

- 実装する: package v2、副作用なし、boolean入力のみの初期subset、boolean出力、永続catalog、固定版の実行、検証・停止・取消・復元。
- 今回は実装しない: MCPサーバー起動、会話ツールへの公開、UI、LLM生成、enum/string/null/undefined入力の公開、任意effect、配布アプリ対応。
- 禁止する: `deny_unknown_fields`やhash検証の削除による互換化、候補内JavaScriptの実行、任意shell、TypeScript実行へのfallback、テスト期待値を実行結果に合わせる変更。
- 新規依存: M1は既存依存を使う。`jsonschema`だけは現行versionと設定を維持してdev-dependencyから通常dependencyへ移す。schema検証を独自の緩い判定へ置き換えない。
- 後続実装へ進まない条件: 今回の範囲を満たすためにABIや対応profileを拡張する必要がある、信頼するruntimeを確定できない、成果物の来歴が特定できない場合。その依存箇所を保留し、無関係な調査・文書化は続ける。

## 1. 根拠として読むファイル

SAAA内のパスはrepository rootからの相対パス。

| 既存ファイル | 確認する内容 |
| --- | --- |
| `src-tauri/src/wasm_host_poc/mod.rs` | 起動、request生成、response検査の順序 |
| `src-tauri/src/wasm_host_poc/contracts.rs` | v1固定のmanifest/report、ID/hash照合、正常false |
| `src-tauri/src/wasm_host_poc/kit.rs` | runtimeとcandidateが一体の信頼記録、fixture制約 |
| `src-tauri/src/wasm_host_poc/process.rs` | 環境の消去、出力上限、EOF、kill・wait、取消 |
| `src-tauri/src/wasm_host_poc/tests.rs` | 実kit、異常応答、ハング、入力上限の既存試験 |
| `src-tauri/Cargo.toml` | `jsonschema`が現在dev-dependencyであること |
| `src-tauri/src/persistence/schema.rs` | migrationのtransaction、schema version |
| `src-tauri/src/persistence/sqlite/writer.rs` | writer closureは同期。通常コードで`write_transaction`は使えない |
| `src-tauri/src/app_state.rs`、`src-tauri/src/lib.rs` | Runtime保持と初期化の位置 |
| `src-tauri/src/providers/stream/dispatch.rs` | M2での提示・実行入口。M1では変更しない |

L-Lang側は`/Users/y.noguchi/Code/L-Lang`を調査時の場所とする。実装へこの絶対パスを埋め込まない。

| L-Lang内の既存ファイル | 確認する内容 |
| --- | --- |
| `src/capability-snapshot.ts` | package v1/v2の読取り分岐 |
| `src/capability-host.ts`、`src/capability-host-cli.ts` | host protocol、operation、candidateの引数 |
| `src/capability-host-kit.ts`、`src/capability-host-v2.test.ts` | v2 kitの生成と再配置試験 |
| `src/llang-capability-contracts.ts` | v2のrequest/source/build/wasm/tests参照 |
| `src/llang-capability-verifier.ts`、`src/llang-case-runner.ts` | v2 reportの構造とpass/fail/error |
| `src/llang-capability-inspection.ts`、`src/llang-predicate-projection.ts` | inspection。調査時は未コミットであり再確認必須 |
| `examples/jsonc-enabled-user/` | 最初のboolean fixtureの元 |

## 2. 採用する内部契約

### 2.1 packageとruntimeの版を混同しない

| 項目 | 初期値・扱い |
| --- | --- |
| package manifest version | `2`のみを新Runtimeで受付。旧PoCのv1は回帰試験用に残す |
| host protocol | 固定版で確認した`llang-host-v1` |
| Wasm profile | `predicate-i32-v1` |
| 入力 | 1〜8個の必須boolean field。nullable/undefinable/optionalはすべてfalse、valuesは空 |
| 出力 | booleanのみ。falseは成功値 |
| permissions | 空配列のみ。未知の要求は拒否 |
| 未対応入力型 | `unsupported-contract`。既存L-Lang全体の対応範囲を狭めず、SAAAの初期subsetと説明する |
| HostへのundefinedFields | 空配列固定 |
| 検証方法 | L-Lang verifyと、SAAA側の独立した受け入れケースの両方 |

Runtimeの実行コード、schema、workerはSAAAが信頼するbundleへ固定する。candidateは別の保存領域へ置く。candidateがruntime entrypoint、Bun path、importするJS、環境変数を指定するフィールドは受け付けない。

### 2.2 初期上限

以下は初期実装の設計値。`limits.rs`に集約し、境界値の試験を作る。必要なfixtureが超える場合、無断で無制限にせず原因を報告する。

| 対象 | 上限 |
| --- | --- |
| Host request全体 | 64 KiB（既存と同じ） |
| stdout / stderr | 1 MiB / 64 KiB（既存と同じ） |
| inspect / verify / invokeのHost期限 | 15秒 / 30秒 / 15秒（既存と同じ） |
| Wasm invokeの内側期限 | 既定1秒、最大10秒。外側期限を越えない |
| 同時Hostプロセス | 1。空きがなければ`busy`、無制限queueを作らない |
| candidate取込み | manifestと参照5ファイル、合計6 MiB以下、各参照ファイル1 MiB以下、manifest64 KiB以下（固定版L-Langの各file上限とも照合） |
| 受け入れケース | 最大256（8 booleanの全組合せ） |
| 受け入れ試験全体 | 60秒。期限後に新しい子プロセスを起動しない |
| metadata purpose/useWhen/doNotUseWhen | 各UTF-8 2 KiB以下 |
| 生stderrの永続保存 | 行わない。サイズ制限した安全なエラーコードを保存 |

この上限はOSレベルのメモリ隔離や完全なsandboxを意味しない。子プロセス以外の任意process treeを生成できるruntimeへ広げない。実行コードの信頼とWasmの検証が前提である。

### 2.3 識別子とデータ

- `capability_id`: SAAAが割り当てるUUID。candidateのmetadata.idと別物。既存IDへの更新は管理側が対象IDを指定する。
- `revision_id`: SAAAが割り当てるUUID。releaseという文字列から大小を推測しない。
- `package_hash`: L-Langの固定版が定義する算出方式を使う。調査時はparse後のmanifestに対する`contentHash(manifest)`で、`prompt-source.ts`のcanonical処理が基準。独自のJSON文字列化やmanifestのbyte hashで代用しない。Rust側の算出は固定したcanonical test vector（キー順、空白、日本語、escapeを含む）と一致させる。
- `inventory_hash`: SAAA管理下へコピーした各ファイルの相対名・byte hashを固定順で記録した一覧のdigest。package_hashとは用途が違う。
- `contract_hash`: 初期subsetのfield名を昇順に並べ、入力型、必須性、profile、出力型を固定順の構造でserializeしてSHA-256。descriptionとreleaseは含めない。方式に`contract_format=1`を付ける。
- `runtime_digest`: 信頼するruntimeの全ファイル一覧のdigest。candidate側の値を信頼設定へ取り込まない。
- `acceptance_hash`: 管理側が固定した期待値一覧のdigest。candidateが自分の採点基準を選べないようにする。

JSONのbyte hashと意味的なcontract hashを混同しない。TypeScript inspection用のsourceHash/programHash/artifactHashは、固定版の検証結果から取得する。nullableにするのは未検証candidateだけで、validated以降は揃っていなければならない。

### 2.4 共通サービスAPI

下記はRust内部APIの責務を固定するための疑似シグネチャ。Tauri commandでもMCP toolでもない。命名はこのまま採用し、既存名と衝突した場合だけ対応表を記録して変更する。

```rust
import_candidate(ImportCandidate) -> Result<RevisionRef, CapabilityError>
verify_candidate(RevisionRef, AcceptanceRef, Cancellation) -> Result<VerificationSummary, CapabilityError>
activate_revision(RevisionRef, expected_catalog_epoch) -> Result<Activation, CapabilityError>
resolve_active(CapabilityId) -> Result<ResolvedCapability, CapabilityError>
invoke(InvokeRequest, Cancellation) -> Result<InvocationResult, CapabilityError>
suspend_revision(RevisionRef, expected_catalog_epoch) -> Result<(), CapabilityError>
reconcile_startup() -> Result<RecoverySummary, CapabilityError>
```

`ImportCandidate`は信頼する管理コードが作り、対象能力ID（新規ならNone）、外部candidate directory、固定した取得元情報、要求に対応するrequired_acceptance_hashを含める。パス入力をLLMやMCPへ直接公開しない。AcceptanceRefはHost側の固定台帳から解決し、revisionに記録したrequired_acceptance_hashとの一致を必須にする。生成候補や生成されたtests.jsonから期待値を逆算しない。

`ResolvedCapability`にはcapability_id、revision_id、package_hash、contract_hash、catalog_epoch、入出力schemaを入れる。`InvokeRequest`はこの解決結果、Host側発行call_id、入力object、inner timeoutを持つ。入力は全必須fieldがあり、余分なfieldがなく、全値がbooleanであることをspawn前に検査する。

`InvocationResult`はcall_id、revision_id、package_hash、value: bool、elapsed_msを返す。失敗時は同じ参照を持つ構造化エラーとする。入力、環境変数、任意の外部pathをエラーメッセージへ複製しない。

## 3. 永続化と状態遷移

### 3.1 テーブル

以下の名前で新設する。既存の会話テーブルやUIテーブルへ能力の正本を混在させない。日時は既存の`now_iso()`の形式を使う。

| テーブル | 必須列と制約 |
| --- | --- |
| `generated_capabilities` | id PK、current_revision_id nullable、catalog_epoch非負整数、created_at、updated_at |
| `generated_capability_revisions` | id PK、capability_id FK、package_hash、inventory_hash、contract_hash、runtime_digest、required_acceptance_hash、provenance_json、manifest_json、contract_json、metadata_json、state、source_hash/program_hash/artifact_hash nullable、created_at。UNIQUE(capability_id, package_hash)、UNIQUE(capability_id, id) |
| `generated_capability_checks` | id PK、revision_id FK、runtime_digest、inventory_hash、acceptance_hash、status、error_code nullable、report_ref nullable、started_at、completed_at nullable |
| `generated_capability_calls` | id PK、revision_id FK、package_hash、origin、status、result_bool nullable、error_code nullable、started_at、completed_at nullable |
| `generated_capability_imports` | id PK、status、revision_id nullable FK、error_code nullable、created_at、completed_at nullable。構造不正でrevisionを作れない失敗も記録する |

`current_revision_id`は`(id,current_revision_id)`からrevision側`(capability_id,id)`への複合FKにする。循環参照のため最初はNULLで能力行を作り、revision挿入後に更新する。`ON DELETE CASCADE`で検証履歴を消さない。初期実装に物理削除APIは作らない。

stateは`candidate / validated / active / suspended / retired`のCHECK制約。`active`行はcapability_idごとに最大一つとなるpartial UNIQUE indexを作る。現在版pointerとactive行の対応は単一transactionで更新し、不整合のDB fixtureを復元試験で検出する。

checks.statusは`running / passed / failed / interrupted`、calls.statusは`running / succeeded / failed / cancelled / interrupted`、imports.statusは`staging / completed / failed / interrupted`。正常falseは`result_bool=0,status=succeeded`。raw inputや生stderrは保存しない。生出力はbooleanだけを保存する。

### 3.2 状態変更の規則

| 操作 | 事前条件 | 成功後 | 失敗時 |
| --- | --- | --- | --- |
| import | 構造・サイズ・path検査が通る | candidate | importのfailedを記録、公開しない |
| verify | candidate、同じrevisionの検証が未実行中 | 合格ならvalidated | candidateのまま、checkにfailed |
| activate | validated、現在runtime/inventoryのpassed checkがある、epoch一致 | 対象active、旧activeはvalidated、pointer切替、epoch+1 | 全rollback、旧activeを維持 |
| suspend | validatedまたはactive、epoch一致 | suspended。現行ならpointer=NULL、epoch+1 | conflictなら状態を変えない |
| revalidate | suspended、管理操作で指定 | 新しいcheck合格後validated | suspendedのまま |
| retire（M3） | active以外 | retired | retiredから再有効化しない |

初回公開を始めるM1の機能フラグがoffならinvoke/activateは`disabled`。import/verifyは明示的な管理・試験入口に限り可能にし、通常起動では自動実行しない。

旧版へ戻す場合もrevalidate/activateと同じ検査を使う。validatedの再検証も許可するが、activeを再検証する場合は先にsuspendする。検証結果を書き換えてpassedにしない。passed checkは対象revision・inventory・現在の信頼runtime・required_acceptance_hashの組合せへ固定する。runtimeを更新した場合は古いpassed checkを流用せず、再検証成功時にrevisionのruntime_digestを更新する。

### 3.3 競合と取消

DB transactionとwriter closureの中で、subprocess待機、ファイルコピー、network、`.await`を行わない。`SqliteWriter::write()`のclosure内で`connection.transaction()`を開始・commitする。テスト限定`write_transaction()`を通常コードから呼ばない。

サービスに短時間のadmission mutexとHost processのSemaphore(1)を持たせる。ロック順は常に `admission → writer`。Host待機中にどちらも保持しない。DB読取り後に、writer側transactionでactive版とepochを再確認する。

invokeは次の順で実施する。

1. 入力・機能フラグ・取消状態を検査する。実行枠がなければbusy。
2. admission下で、active版・revision/hash/contract/epochを再検査する。古ければ`stale-revision`、停止済みなら`not-active`。
3. calls.runningをcommitする。このcommitを「実行を受け付けた時点」とする。commitに失敗したらspawnしない。
4. ロックを解放し、信頼済みruntimeとcandidateの整合性を再確認して起動する。
5. 成功・失敗・取消の終了記録をwriterで保存し、process枠を解放する。

activateとsuspendも同じadmissionを通す。受付済みの呼び出しは旧版のまま完了してよい。suspendは以降の受付を止める操作と定義し、実行中の中断はCancellationで行う。suspend完了を「実行中processも停止した」と報告しない。

verifyは開始時にcheck.runningをcommitし、終了時にrevisionのstateと元のinventoryを再検査する。検証中に停止・廃止された場合、合格結果を記録してもstateをvalidatedへ戻さない。同revisionの二重検証はbusyとする。

検証専用の実行順は次に固定する。

1. AcceptanceRefのdigestとrequired_acceptance_hashを照合し、runtimeとinventoryを再検査する。
2. 共通Semaphoreの枠を取得し、check.runningを記録する。検証全体が終わるまで枠を保持する。
3. Hostのverifyを呼び、構造検査と各case結果を検査する。不合格なら独立ケースの実行へ進まない。
4. SAAAの独立ケースを同じpackage/hashに対して順に実行する。公開`invoke()`はactive状態を要求するため呼ばず、verificationモジュールだけが使えるprivate Host実行関数を通す。この関数でSemaphoreを再取得しない。
5. 各caseの前に取消と全体deadlineを確認する。個別期限は残り時間以下にする。一件でも不一致・timeoutなら打ち切る。
6. reportをSAAA管理領域へ保存し、そのdigestをreport_refに含める。保存失敗ならvalidatedへ進めない。
7. writerで元のstate・inventoryを照合し、checkと必要なstate変更を一transactionで確定する。枠を解放する。

呼出元が切断してもprocessを所有する管理taskを放置しない。取消を伝え、kill/wait完了まで回収処理を継続する。futureをdropするだけで回収済みとみなさない。終了記録の保存に失敗した場合は`storage-error`とし、成功を返さず、running行は次の起動でinterruptedにする。

## 4. 保存領域と取込み手順

```text
<SAAA data directory>/generated-capabilities/
  staging/<import-id>/
  packages/<package-hash>/
  reports/<check-id>.json
  inspections/<revision-id>/       # M3で追加
```

SAAA data directoryは既存database pathのparentから得る。候補のmetadataや入力から構築しない。M1の開発設定はSAAAプロセス起動側が指定する`SAAA_LLANG_RUNTIME_CONFIG`（信頼設定JSONへの絶対path）とし、候補由来の環境変数で上書きしない。未設定なら機能disabledで通常のSAAA起動を継続する。

信頼設定JSONはformatVersion、enabled、bunPath、runtimeRoot、expectedRuntimeDigestを持つ。runtimeRootのmanifestは固定したファイル一覧とhash、採用L-Lang版、Bun版、entrypointとschemaを持つ。expectedRuntimeDigestはT02で得た固定値を管理者側が設定する。起動時に自動計算した値をその場で信頼値として採用しない。設定は秘密情報を含めず、既存ユーザー設定へ自動登録しない。

取込み手順:

1. imports.stagingをDBへ記録し、SAAA生成のimport-idでstaging directoryを作る。
2. 外部manifestをサイズ制限付きで読む。初期版は参照pathをflatなファイル名に限定し、絶対path、`..`、separator、重複、symlink、通常file以外を拒否する。対応できない正当なpackageも`unsupported-package-layout`として明示的に拒否する。
3. manifestと参照された5ファイルだけをstagingへbyte copyする。追加JSや候補のruntime directoryをコピーして実行しない。copy後に管理コピーを基準として全byte hashを照合する。
4. 信頼済みHostによるinspectをstagingに対して行い、profile/contract/permissionsとpackage_hashを確認する。package_hashの事前算出はT01で固定したL-Lang方式で行う。
5. SAAA inventoryを生成し、同filesystem内でpackages/<package-hash>へrenameする。既存directoryがあれば上書きせず全内容を照合し、一致時だけ共有する。違えばintegrity-error。
6. writerの一transactionで能力行・candidate行・import completedを保存する。同能力に同package_hashが既存でrequired_acceptance_hashも一致するなら同じrevisionを返し、検証や有効化を勝手に再実行しない。acceptanceが異なる場合はconflictとして、既存revisionの採点基準を書き換えない。

外部sourceは信頼しないが、任意の同一OSユーザーによる管理領域への能動的な書換えに対する完全隔離はM1の保証外。読み取り専用属性だけで安全と判断せず、検証・実行前に再照合し、Hostも読み取ったsnapshotを実行することをT01で確認する。

起動時の回復はDBとfileの確認だけを行い、Wasmを実行しない。

| 中断箇所 | 回復 |
| --- | --- |
| staging作成後 | importをinterrupted。残存stagingは再利用しない |
| final rename後、DB commit前 | orphan packageとして検出。自動登録・自動削除しない |
| check/callがrunning | interruptedへ変更。自動再試行しない |
| active package欠落・改変 | suspendedへ変更、pointerを消しepoch+1 |
| runtime不在・digest不一致 | サービスunavailable。既存会話の起動を妨げない。別runtimeへ自動fallbackしない |
| pointerとactive行が不一致 | その能力を停止して整合性エラーを記録。推測でcurrentを選ばない |

## 5. M0の作業カード

各カードの成果物を`spec/docs/verification/llang-dynamic-capability-m0.md`へ記録する。このファイルは実装担当が実測後に作成する。今回の計画作成時点では作らず、空の成功記録を置かない。

### T00 — baselineと依存版の記録

- 変更: 検証記録のみ。
- 実施: SAAA/L-LangのHEAD、dirty paths、Bun/Rust版、既存PoC/full gate結果を記録する。秘密情報は出力しない。
- 完了: 未追跡のinspectionを含むsnapshotを採用するなら、commitとは別に全対象fileのhashを固定する。dirty:falseと偽って記録しない。

### T01 — v2 wireの比較とfixture設計

- 変更: 比較表、固定したresponse JSON fixture、fixture生成手順。
- 比較必須: manifestのv1 `lock`とv2 `request`の差、verifyのversion/verifier、case結果の形、hash群、coverage、diagnostics。
- 採用するv2 verifierは固定版で確認した`llang-capability-v2`。v1のpassed/failed/errors集計やoriginをv2に要求しない。
- outer status=okでも、verify.status=fail/errorは不合格。caseのexpected/actual/status整合、未知case、重複ID、未充足requirement、diagnosticsを検査する。空caseを合格にしない。
- 完了: 生responseをfixture保存し、Rust型の変更項目を列挙する。存在しないフィールドを埋めた「想定response」は使わない。

### T02 — runtimeとcandidateの分離、M0-R判定

- 変更: 再現可能なruntime bundle作成手順と信頼manifest、v2候補A/B。
- 方式: 固定したL-LangからHost CLI・worker・schemasをbundleし、runtimeだけのmanifestをSAAA管理用として作る。L-Lang package形式は変更しない。既存kit生成物からruntimeを抽出する場合、必要workerとschemaを漏らさず記録する。
- 実施: 同じruntime digestでcandidate AとBを切り替えてinspect/verify/invokeする。空白を含む別directoryへ移しても実行できることを確認する。
- 完了: 両candidateで動作し、source checkoutや認証情報なしに実行できる。runtimeの改変と未知entrypointを拒否する。M0-R=PASSとする。
- 停止: 同一runtimeで二候補を扱えない、candidate側JSを起動する必要がある、snapshotのhashと実行対象が対応しない場合。これらを未検証のままM1へ渡さない。

### T03 — inspectionの依存確認、M0-I判定

- 実施: 固定版のinspectionがcandidate Aに対応するTypeScriptとhashを返すか調べる。CLI入口の有無と出力サイズ・エラー形式を記録する。
- 完了: 実際の出力と対象package/program/artifact/projection hashを照合しM0-I=PASS。未提供ならM0-I=BLOCKED、M0全体=PARTIALとする。
- 継続範囲: M0-R=PASSならM1/M2は進められる。M3のinspectionとM4全体完了は待つ。

## 6. M1のファイル配置と作業カード

### 6.1 配置

`src-tauri/src/generated_capabilities/`を新設する。下記は責務の分割であり、初日から空fileを全件作る必要はない。

| ファイル | 責務 |
| --- | --- |
| `mod.rs` | 必要な内部APIのexportだけ |
| `contracts.rs`、`errors.rs`、`limits.rs` | SAAA内部型、安定エラーコード、初期上限 |
| `host/mod.rs`、`host/wire.rs`、`host/runtime_bundle.rs` | v2 request/response、runtime信頼、起動組立て |
| `host/process.rs` | 既存process処理を移す。二重実装しない |
| `package_store.rs` | staging/copy/inventory/rename、管理root境界 |
| `schema.rs`、`repository.rs` | migrationと短いDB操作 |
| `verification.rs` | verifyと独立acceptanceの実行・結果照合 |
| `service.rs`、`lifecycle.rs` | admission、invoke、有効版切替と停止 |
| `recovery.rs` | 起動時のinterrupted化と整合性検査 |
| `tests/` | contract、process、store、lifecycle、recovery、serviceの試験 |

既存PoCのprocess.rsを新しい場所へ移し、旧PoCから同じ実装を参照する。v1専用contracts/kitと旧fixtureは回帰用に残す。`#![allow(dead_code)]`を新モジュール全体にコピーしない。未使用の後続APIは必要になるまで実装しない。

### T10 — 型とv2 wire parser

- 前提: M0-R=PASS。
- 変更: contracts/errors/limits、host/wire、Cargo.tomlのjsonschema依存位置。
- 実施: 生response fixtureを使って型を定義する。操作別型、既知version、request ID、package hash、`apiCalls=0`、boolean出力を厳格に検査する。
- 完了試験: W01〜W05。通常`cargo check`でもjsonschemaを参照できる。v1の既存試験を緩めない。

### T11 — process再利用と固定runtime起動

- 変更: host/processとruntime_bundle、旧PoCのimport。
- 実施: 旧processを挙動を変えずに移し、T02で固定したruntimeからだけ起動する。command_overrideはテスト専用にする。
- 完了試験: H01〜H04と既存PoC全試験。runtimeとcandidateのdigestが別々に検査される。

### T12 — 管理保存領域とcandidate取込み

- 変更: package_store、schema、repositoryのimports/revisions処理、`persistence/schema.rs`。
- 実施: 先に第3節のschemaを作り、既存migration transaction内から呼ぶ。調査時の全体versionは20だが、実装時点の最新versionを確認して未使用の次versionを採用する。固定で21へ上書きしない。
- 実施: DBの用意後に第4節の取込み順序を実装する。外部directoryへの書込みや再帰的な無制限copyをしない。
- 完了試験: D01〜D03、P01〜P05。candidate A/Bの切替でruntime bundleが変わらない。

### T13 — 検証と有効化

- 変更: repositoryの状態変更、verification、lifecycle。
- 実施: SAAA独立acceptanceはpackageのtests.jsonとは別の固定ファイルから読み、管理側が能力要求へ結び付ける。candidateはAcceptanceRefを指定できない。
- 完了試験: V01〜V04、L01〜L04。DB lockを持ったままHostを待たない。

### T14 — invokeと回復

- 変更: service、recovery。
- 実施: 第3節の受付・実行・終了記録を実装する。確認直後の更新、停止、取消をテスト用barrierで再現する。sleepの長さに依存する競合テストにしない。
- 完了試験: I01〜I06、R01〜R04。startupからinvoke/verifyを呼ばない。

### T15 — AppStateへの接続とM1統合

- 変更: `app_state.rs`、`lib.rs`、必要な既存AppState test constructor、module-size登録。
- 実施: `Arc<CapabilityService>`をAppStateから利用できるようにし、既存writerを渡す。Runtime設定がない場合はdisabled serviceで起動する。M1の管理操作はRust内部APIと試験harnessのみで呼び、Tauri/MCP/会話の新入口を追加しない。
- 実施: 起動時のrecoveryは既存DB初期化後に行う。終了時には所有する実行へ取消を伝え、回収結果を扱う。起動時・終了時にcandidateを自動実行しない。
- 完了: 通常buildに実装が含まれ、テストだけのモジュールではない。fixture特有の分岐がproductionにない。全M1試験と既存gateを通す。
- 注意: `bun run size:register`は新規fileの登録だけに使い、既存fileの上限を書き換えて失敗を隠さない。

## 7. fixtureと期待値

新規fixtureは`src-tauri/tests/fixtures/llang-capability-v2/`へ置き、`runtime/`、`candidate-a/`、`candidate-b/`、`acceptance/`、`wire/`に分ける。生成元・固定版・hash一覧・再生成コマンドをREADMEへ残す。今回の計画ではこれらのfileをまだ作らない。

候補Aは`enabled && !suspended`。候補Bは同じ入出力契約で`enabled`のみを見る。Bは「停止フラグによる除外を外す」という試験上の仕様変更であり、実際のアクセス制御には利用しない。同能力の別版としても、別IDの新規能力としても取り込めることを検査する。

| enabled | suspended | A | B |
| --- | --- | --- | --- |
| false | false | false | false |
| false | true | false | false |
| true | false | true | true |
| true | true | false | true |

文字列`"true"`、null、必須field欠落、余分なfieldはspawn前にinvalid-input。候補のtest suiteにも正例・負例を含めるが、上表はSAAA側の独立acceptanceとして別管理する。

破損fixtureは毎回tempdirへのコピーで作り、repositoryの正常fixtureを書き換えない。hash不整合の拒否試験と、hashが整合するが許可されないprofile/importを持つ候補の拒否試験を分ける。後者は実際のL-Lang検証器で拒否されたことを確認する。

無限実行Wasmがprofile検証を通らない場合、検証規則を緩めない。「不正Wasmは起動前に拒否」と「テスト専用のハングHostをkill/wait」の二つで境界を試験し、どこを検証したか報告する。

## 8. M1の試験一覧

各IDをRust test名またはtestコメントに残し、最終報告で対応を示す。

| ID | 入力・操作 | 合格条件 |
| --- | --- | --- |
| W01 | 実v2 inspect/verify/invoke JSON | strict parserで読める |
| W02 | protocol、ID、hashを各一つ改変 | 応答を拒否 |
| W03 | invokeのvalueを文字列falseへ変更 | 成功値としない |
| W04 | outer ok、inner fail/error、case結果矛盾 | 検証合格にしない |
| W05 | 未知field、未知version、空case | エラー。v1型へfallbackしない |
| H01 | 同runtimeでA/Bを実行 | 上表と一致、runtime digest不変 |
| H02 | runtime file改変、Bun不在 | candidateをspawnせず明示エラー |
| H03 | ハングHost、取消、出力超過 | kill/waitしreaped確認、後続正常呼出し成功 |
| H04 | credentialを親環境へ設定してテスト | 子のenvへ渡らない。値をログに出さない |
| P01 | 同能力へ同package再import | 同revision、state不変、二重登録なし |
| P02 | 外部sourceをimport後に変更 | 管理コピーの実行結果不変 |
| P03 | symlink、../、絶対path、過大file | stagingからの公開なし、失敗記録あり |
| P04 | 管理copyを改変 | verify/invoke/activateで拒否 |
| P05 | 同hashの既存保存領域が異なるbyte | 上書きせずintegrity-error |
| D01 | 空DB初期化・再初期化 | table/index/FKが揃い冪等 |
| D02 | 変更前schemaのDBをmigration | 会話等の既存データ保持 |
| D03 | 別能力revisionをcurrentへ指定 | FK拒否、一能力二activeも拒否 |
| V01 | Aと正しいacceptance | validated、対応hash記録 |
| V02 | AにBの期待値をrequiredとしてimportした試験候補、またはverify時だけ別hashを指定 | 前者はfalse/true差で不合格、後者はhash不一致で拒否。どちらもcandidateのまま |
| V03 | verify中の停止、二重verify | 停止を解除しない、二重実行なし |
| V04 | verify期限・acceptance全体期限 | 実行回収、合格扱いしない |
| L01 | 検証前・failed・旧runtime検証でactivate | 拒否、現行pointer不変 |
| L02 | A activeから合格Bへ切替 | Bのみactive、epoch+1、Aはvalidated |
| L03 | 二つの切替が同epochを指定 | 一方だけ成功、他方conflict |
| L04 | transaction途中で障害注入 | pointer/state/epochすべてrollback |
| I01 | 全真理値表と入力不正 | 値一致、不正入力はspawn回数0 |
| I02 | 古いResolvedCapabilityでinvoke | stale-revision。新しい版に転送しない |
| I03 | 受付前/後でsuspendをbarrier同期 | 前は拒否、後は固定版で終了可 |
| I04 | 受付後の取消、caller切断 | cancelled、process回収、枠解放 |
| I05 | capacity使用中に追加invoke | busy、無制限queueなし |
| I06 | 開始/終了DB書込みを失敗させる | 開始失敗ではspawn0、終了失敗では成功を返さない |
| R01 | running check/call、staging importを残して再起動 | interrupted、spawn0 |
| R02 | active payload欠落・改変 | suspended/currentなし、他の会話は利用可 |
| R03 | rename後DB commit前の中断 | orphan検出、自動公開なし |
| R04 | 正常登録後に再起動 | catalogと版を保持、復元時spawn0 |

安定エラーコードは`disabled / unavailable / unsupported-contract / unsupported-package-layout / invalid-package / invalid-input / integrity-error / verification-failed / not-validated / not-active / stale-revision / conflict / busy / timeout / cancelled / output-limit / protocol-error / storage-error`を初期集合とする。未知の内部例外を成功やfalseへ変換しない。

## 9. 実行するコマンドと合否の記録

SAAA rootで実行する。新しいtest filterは実装後にtest一覧を見て0件でないことを確認する。

```sh
cargo test --manifest-path src-tauri/Cargo.toml wasm_host_poc --no-fail-fast
cargo test --manifest-path src-tauri/Cargo.toml generated_capabilities -- --list
cargo test --manifest-path src-tauri/Cargo.toml generated_capabilities --no-fail-fast
cargo check --manifest-path src-tauri/Cargo.toml --lib
cargo fmt --check --manifest-path src-tauri/Cargo.toml
bun run size:check
```

上記を対象変更に応じて実行し、M1統合時に`bun run check:local`を実行する。`#[ignore]`やskipを増やして合格扱いしない。Bun不在なら実kit試験は未実施であり、mockだけの合格をM1完了にしない。

成果報告は以下の形にする。

```text
M0-R: PASS / BLOCKED
M0-I: PASS / BLOCKED
M1: PASS / PARTIAL / BLOCKED
採用SAAA/L-Lang版・runtime digest:
実装済みカード:
未完了カードと理由:
試験ID → test名 → 実行結果:
実kit検証とmock検証の区別:
変更前からの失敗:
新しい制約・未対応項目:
次に実装可能な段階:
```

## 10. 後続M2〜M4の作業分割

この節は後続の依頼を小さく切るための仕様であり、M1の担当AIが続けて実装する指示ではない。MCPのwire仕様、モデルproviderの構造化生成契約などは、その段階の着手時に一次仕様と固定した実装で確認し、未確定事項を解消してから変更する。

### M2-A — 会話ツール

- 入口: 既存`available_agent_tools`と`execute_agent_tool`。toolが実際に提示されたかの既存検査を維持する。
- 名前: `gc_<revision UUIDのハイフンを除いた32文字>`。metadata.idをそのまま名前にしない。名前からrevisionを解決するDB索引を使い、prefix一致だけで実行しない。
- schema: 初期subsetからobject/properties/required/additionalProperties=falseを作る。型変換せず、文字列booleanを拒否する。
- description: metadataは機能の用途を示すデータとして扱い、system instructionへ連結しない。用途説明に権限や実行先の決定を委ねない。
- 提示: 最初はactiveのうち明示許可された最大8件、説明とschema全体32 KiB。超過分を暗黙に切り捨てず、管理側の公開設定で絞る。
- 固定: provider requestごとに提示したResolvedCapabilityを保持し、返ってきたtool callはその集合と照合する。未知名をrecall実行へ落とさない。stream parserやprovider固有allowlistも検索して対応する。
- 試験: fake providerでAの定義提示→call→boolean結果、未提示tool拒否、応答途中の版切替、既存recall/UI/coding toolの回帰。
- 停止: 一つのprovider経路だけ直して全provider対応済みとしない。対応経路を列挙し、未対応は明示する。

### M2-B — MCP接続口

- 配置: `generated_capabilities/mcp.rs`を起点とするSAAA内adapter。AppStateと同じArc<Service>を受け取る。別processのwriterやcatalogを作らない。
- 公開: `127.0.0.1`へbind。明示的な開発設定があるときだけ起動する。認証tokenは起動側から受け取り、LLM contextやログへ出さない。
- 初期認可: 単一ローカル所有者の明示許可済みactive集合のみ。MCP接続だけで管理操作を許可しない。
- 実装前成果物: 採用MCP protocol version、初期化、tools/list、tools/call、取消、エラー、HTTP制限の対応表。現在のreasoning MCPをコピーしただけで準拠済みとしない。
- 初期動作: 再一覧取得で追加・停止を反映。自動list change通知は最初は対応を宣言しない。UUIDベースの版付き名のため、旧名callを新しい版へ転送しない。
- 試験: protocol clientからinitialize/list/call、無認証、Origin付き要求、未知名、不正入力、取消、追加後の再list、停止後の旧名call。会話経路と同じrevision/hash/valueを検証する。

### M3・M4 — 統合計画へ移管

M3-A、M4-A、M4-Bの実装指示・予算・比較試験・実モデル実証は[実行版検査・動的生成 統合実装計画](saaa-llang-generation-inspection-plan.md)へ集約した。重複した指示は削除し、以後は同書のT00〜T09・R1/R2を使用する。M0-Iの依存確認とM0/M1の実装記録は本書に残す。

## 11. 実装担当へ渡す開始指示

> 初期計画と本手順書に従い、M0/M1だけを実装してください。最初に既存変更とbaselineを記録し、T00〜T03で接続契約を確認してください。M0-Rが合格したらT10〜T15を順に実施してください。M0-Iが未達でも独立したM1は進めて構いませんが、inspectionは未完了と報告してください。候補と信頼済みruntimeを分離し、既存PoCのprocess管理を再利用してください。会話・MCP・UI・LLM生成には進まず、試験IDごとの結果と残る依存を報告してください。既存ファイルの変更を取り消したり、検証を緩めたりして完了扱いにしないでください。
