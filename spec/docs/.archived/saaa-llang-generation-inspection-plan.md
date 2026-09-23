# SAAA × L-Lang — 実行版検査・動的生成 統合実装計画

更新日: 2026-09-20 / v2 / 未実装M3・M4の正本

## 1. 目的と範囲

既存の能力実行・検索・MCP基盤に、実行した版のTypeScript検査と、要求から能力を生成・更新して利用する流れを接続する。初期計画のM3、詳細手順のM3-A/M4-A/M4-Bと受入試験AC8〜AC11を本書に移した。旧文書の同項目は本書への参照に置き換え、実装指示を二重管理しない。

今回の成果は「新規要求 → L-Lang生成 → 独立検証 → 有効化 → SQLite検索台帳への反映 → 会話/MCP利用 → 実行版のTypeScript取得 → 変更要求による更新」。単に既存packageを選ぶことを生成の完成としない。

今回は既存 `predicate-i32-v1`、1〜8個の必須boolean入力、boolean出力、外部effectなしに限定する。汎用入出力、host API、OpenUI binding、配布runtimeの自動同梱、WASIは未実装の別段階として末尾に集約する。これらを今回達成したとは報告しない。汎用自由度を求めるコンセプトは維持する。

既存M1のimport/verify/activate/suspendとD0〜D5の検索・認可・実行を再利用する。新しいwriter、独立catalog、MCP管理tool、任意shell実行、TypeScriptによる通常実行fallbackを追加しない。

## 2. 現状と依存の確認

`generated_capabilities/service.rs` にimport・検証・有効化・停止・実行はある。`tool_selection` に検索・条件付き訂正・MCP接続/公開がある。一方、通常経路の実行履歴からTypeScriptを取得する接続と、LLM生成から登録する接続が残っている。既存の版切替を再実装せず、不足する公開経路と試験を追加する。

[M0検証報告](verification/llang-dynamic-capability-m0.md)のM0-I=PASSはL-Lang固定snapshotでinspectionが動いた記録であり、SAAAの通常サービスへ接続済みという意味ではない。実装開始時に固定版・digest・CLI/ライブラリ入口・v2 package生成を再確認する。L-Lang側の不足は再現入力と必要contractを記録し、無断で別repositoryを変更しない。

T00で、現在HEAD、未コミット差分、DB schema version、採用L-Lang/Bun/runtime digest、inspection format、生成/build入口、対応providerを `spec/evidence/llang-generation/dependencies.md` に記録する。既存の未コミット変更をstash/reset/cleanで退避・削除しない。

## 3. 実行版のTypeScript検査

内部API `inspect_execution(principal, invocation_id)` を設ける。通常会話・tool_selection双方の実行IDを、永続化した対応表からgenerated capabilityのcall/revisionへ解決する。文字列からIDを推測したり、現在activeの版を参照したりしない。対応は実行前に保存する。具体的なID対応と所有者記録は12章で固定する。

処理順:

1. invocation所有者・project等の閲覧権限を確認する。閲覧権限は実行権限と区別し、suspend/retire済みの過去版も所有者が調査できるようにする。他人のIDはnot-found相当で拒否。
2. 実行時revision、package/source/program/artifact/contract hashを取得し、保存packageのbyte hashを再検査する。
3. 固定した信頼inspection実装を実行する。candidate由来JSをentrypointにしない。既存hostのprocess取消・環境消去・出力制限を再利用する。
4. TypeScriptとreportを管理領域へ保存し、取得時にもhashを確認する。欠落、未生成、不一致、未対応runtimeを別エラーにする。LLMの書き直しによる代用は禁止。
5. 初版の表示は12.7の会話messageに固定する。TypeScriptとreportのダウンロードUI・専用管理画面は作らない。

保存単位は `(revision_id, inspector_digest)`。同revisionのinspectionを再生成して過去の証拠を上書きしない。新規table `generated_capability_inspections` は12.4のDDLに従い、完全な成果物だけを記録する。pathはホスト生成で、packageの相対pathをそのまま出力先にしない。DB versionは着手時の最新版+1。fileをstaging→atomic rename→DB確定の順で保存し、再起動時に未確定物を公開しない。

上限はinspection全体15秒、TypeScript/report合計1MiB。超過は切り詰めて成功にせず失敗。出力に `semanticEquivalence: not-checked` 等の元reportの限界を残す。

比較試験は最大8booleanの全組合せ（最大256ケース）。信頼するprojection生成器の出力だけを隔離processで実行し、Wasm結果と比較する。候補内の任意TypeScriptを評価する汎用APIは作らない。結果は `checkedCases`, `mismatches`, `inputDomain`, `projectionHash`, `artifactHash` を保存し、「指定したboolean入力域で一致」と表示する。一般の意味同値性証明とはしない。

## 4. 更新・停止・復帰

既存activate/suspend/epochを使い、会話・MCPに別のcurrent pointerを置かない。非activeのrevisionのみretiredへ遷移できる管理APIを追加する。retireは新規選択・実行対象から外す操作であり、package・実行履歴・inspectionを物理削除しない。

更新は元revisionとcatalog epochを固定して開始する。新候補失敗・同時更新conflictなら旧activeを維持する。互換性はcontract_hash完全一致だけで判定し、入力の追加・削除を推測で互換扱いしない。旧版復帰は現在の信頼runtime/acceptanceで再検証してからactivateする。

能力catalogとtool_selection台帳を接続する同期処理を明示的に追加/再利用する。安定tool_idはcapability_idへ対応させ、backend bindingは実際のgenerated revisionへ固定する。activate/suspend/retire後に台帳を更新し、schema・usage・catalog epochを反映する。自動grantは行わない。

両台帳・grant・generation job終端は同一SqliteWriterの1 transactionで更新する。transaction内で使える関数へ既存guardを分離し、writerの入れ子呼出しは禁止する。outboxは作らない。失敗時は全rollbackし、旧参照を最新版へ転送しない。古い検索行が残ってもbackendのrevision/state再検査で停止済み能力の実行を拒否する。

## 5. 制限付き生成contract

ホスト内部の `GenerationRequest` はrequest_id、principal/project、用途、boolean入力契約、期待判定の説明、acceptance_id、変更時base_revision_idを持つ。credential、任意起動コマンド、runtime path、出力directoryをモデルへ指定させない。初版は事前登録した評価要求と独立acceptanceを対象とする。曖昧な自由文に自動で正解を付けない。

`GenerationService` が既存providerの構造化出力経路へ1回依頼し、versioned L-Lang入力を取得する。buildは固定したL-Lang入口をホストが起動する。generatorの戻り値にはformatVersion、生成source、採用contract、診断を持たせる。モデル応答と実在CLIへの変換は12章の固定contractに従う。存在しないCLIを推測して実装しない。

初期予算: model call 1回、max output 4096 tokens、モデル生成からbuildまで壁時計120秒、モデル出力64KiB。packageは既存M1の上限を適用する。timeout/取消で生成・build processを回収し、予算拡大や自動修復retryを行わない。消費tokenが取得できなければunknownと記録する。

生成状態は `requested → generating → building → importing → verifying → awaiting_activation → active`。失敗はfailed、取消はcancelled、同時更新はconflict。状態とerror_code、生成物hash、元revision、usageを `generated_capability_generation_jobs` に記録する。secretは記録しない。再起動時は12.6の状態別規約で回復し、モデルcallや副作用を自動再送しない。

生成用workspaceと実行package領域を分離する。provider credentialは生成要求の送信経路だけで使用し、build/inspection/invokeには渡さない。候補はM1 import→verifyを通す。候補自身が作ったテストに加え、事前に固定した独立acceptanceを必須とする。生成後にテスト期待値を書き換えない。

Host policyが許可済みの副作用なしprofileと完全一致contract、独立検証pass、scope/epoch一致を確認した場合にactivateできる。毎回の人間コードレビューを必須にしない。ただし生成・active化と利用grantは別であり、既存許可を広げない。新規能力へのgrantはホストの生成policyが事前に許可した要求scopeに限り独立の認可操作として記録する。

## 6. 会話との接続と完了の意味

内部の生成受付から通常会話へ到達する経路を設ける。初版は12章の明示コマンドでホスト登録済み要求IDを指定し、GenerationServiceを起動する。LLMによる自動選択は追加しない。検索no_matchだけで無条件生成しない。対象外profileや独立acceptanceなしならunsupportedと返し、既存Coding Agent等への案内に留める。

会話側に必要な生成受付を追加しても、D5の外部MCP公開は3入口のままにする。外部からgeneration/activate/grantを呼べるようにしない。生成jobがactiveかつ台帳同期・認可完了後に通常のsearch→describe→invokeを行う。モデルへ候補packageを直接実行させない。

変更要求は元revisionを固定して別hashの候補を生成する。初回の能力と変更版について会話およびMCPで同じ版を実行し、実行履歴からTypeScriptを取得する。既存fixtureの選択や手書きpackageへの置換でlive生成を成功扱いしない。

## 7. 実装カード

| ID / 依存 | 実装箇所・作業 | 完了条件 |
| --- | --- | --- |
| T00 / なし | 既存状態・依存入口・試験baselineを記録 | M0-I記録の再確認、生成/build v2互換、変更範囲を確定 |
| T01 / T00 | `generated_capabilities/inspection/contracts.rs`、`generation/contracts.rs` | request/result/error/schema/固定上限をfixtureで検証 |
| T02 / T01 | schema/repository、実行ID対応、inspection/job永続化 | 既存DB upgrade、FK正常、二重登録防止、未確定fileの回復 |
| T03 / T02 | `inspection/service.rs` と信頼host adapter | 過去revision固定、所有者検査、改変/欠落/timeout拒否 |
| T04 / T03 | projection比較harness、既存artifact/IPC接続 | 256ケースまで比較、実行履歴からTS/report表示、意味証明と区別 |
| T05 / T02 | lifecycle/guards、catalog同期またはoutbox | retire/復帰、A実行→B更新→A検査、失敗時A維持 |
| T06 / T01,T02 | `generation/service.rs`、fake generator、固定builder | 正常/構文不正/予算/取消/conflict、失敗時非active |
| T07 / T05,T06 | import/verify/activate・独立acceptance・grant policy接続 | 近道なし、権限拡大なし、台帳同期完了後のみ利用可能 |
| T08 / T04,T07 | 会話オーケストレーターと既存provider接続 | 会話要求からjob→3入口→実行→inspection、MCP管理公開なし |
| T09 / T08 | 実モデルで新規要求と変更要求を実証 | 異なる生成hash、独立ケース成功、会話/MCP/TS確認 |
| R1 / T09 | 初回snapshot固定、全差分の自己レビュー | 認可・revision・取消・DB/file整合を経路横断で確認 |
| R2 / R1 | 修正・回帰・結果報告 | 未達と依存を隠さず、修正前後の根拠を提出 |

T00で外部依存不足が分かった場合は依存contractを記録し、独立するカードだけ進める。stubやfixtureのみでT09完了とはしない。巨大なservice.rsへの追記を避け、上記の責務単位に分離する。

## 8. 受入試験

| ID | 期待結果 |
| --- | --- |
| G01（旧AC8） | A実行中にBへ更新してもAのrevision/hashで終端。B検証失敗ならA維持 |
| G02（旧AC9） | B更新後もA実行IDからAのTS/reportを取得。欠落/改変/別principalは拒否 |
| G03（旧AC10） | 全boolean入力のprojection/Wasm比較。不一致をpassにしない。比較域と未証明を表示 |
| G04 | activeのretire拒否、suspend→retire、旧版の再検証→復帰。過去履歴は保持 |
| G05 | 新規生成と変更生成がM1検証を通り、安定tool_idの異なるrevisionとして検索可能 |
| G06 | 構文不正、未知profile、未知import、acceptance失敗ではactive/grantなし |
| G07 | 生成timeout/取消/caller abort/再起動でprocess回収とjob終端。自動再生成なし |
| G08 | 同時更新をbarrierで再現しepoch conflictを拒否。旧executionRefを新版へ転送しない |
| G09 | DB/file/台帳同期の各境界に失敗を注入。再起動後に半端な能力を公開しない |
| G10 | 別project/別userの生成job・inspection読取拒否。認証情報を生成物/実行環境へ渡さない |
| G11（旧AC11） | 実モデルの新規要求＋変更要求を会話/MCPから実行し、その版のTSを取得 |
| G12 | 元の67件等の過去件数に依存せず、現在のgenerated/tool_selection/provider回帰を実測 |

## 9. 検証・提出

開始時と終了時に `cargo test --manifest-path src-tauri/Cargo.toml --lib generated_capabilities`、同 `tool_selection`、同 `providers` を実行する。変更に応じてIPC/artifact表示の試験を追加し、最終にcargo fmt/clippy、`bun run size:check`、`bun run ipc:check`、`bun run check:local` を実行する。失敗をignored化・上限緩和で通さない。

成果報告は `spec/docs/verification/llang-generation-inspection.md`。開始/初回/修正後snapshot、T/G項目ごとの根拠、採用依存hash、migration、生成jobと実行IDの対応、実モデル名/usage/latency、独立ケース、自己レビュー指摘・修正、未達を記載する。実モデル生成とfake generatorの試験を分ける。

## 10. 今回に含めない残件の集約先

以下は従来「後続」とされていた項目の一覧であり、詳細実装カードは未作成。本書に残して追跡し、今回の完了時に削除しない。

| 残件 | 次に固定すべきcontract |
| --- | --- |
| 汎用Wasm入出力・複数profile | 文字列/配列/構造化値、memory所有、serialization、型互換とサイズ上限 |
| SAAA host API・外部effect | ファイル/DB/通信/他toolの権限、取消、結果不明、再実行規約 |
| OpenUIとの能力binding | state/actionとrevisionの対応、再表示時の副作用再実行防止 |
| 配布runtime同梱 | 対応OS/CPU、Bun/信頼bundleの固定・署名・更新・クリーン端末試験 |
| WASI互換 | 必須前提にしない。必要な生成物のimport/ABIが判明した時点で採否判断 |

## 11. 実装担当への依頼文

> 本書を未実装M3/M4の唯一の実装指示として、12章の固定contractと13章の小カード順に実装し、最後にR1/R2を進めてください。既存M1/D0〜D5を再利用し、実行版に対応したTypeScript検査と、限定された要求からのL-Lang生成・更新を通常会話/MCP利用まで接続してください。採用依存を固定し、独立acceptance、revision固定、生成予算、権限分離を守ってください。初回snapshotを残して自己レビュー・修正・再検証してください。旧計画のM3/M4を別仕様として実装せず、汎用ABI等の後続項目を今回完了と報告しないでください。

## 12. 実装時に選択しない固定contract

本章はv1に残っていた選択肢を確定する。前章の概要より本章の具体値・手順を優先する。以下の新API・SQLは実装指定であり、既存実装があるという意味ではない。

### 12.1 確認済みL-Lang入口と依存固定

確認対象: `/Users/y.noguchi/Code/L-Lang/src/llang-cli.ts`、`src/llang-capability-inspection.ts`、`docs/LLANG_CLI_REFERENCE.md`。以下の入口が存在する。

```text
bun run llang package <source.llang.jsonc> --request <request.json> --suite <tests.json> --metadata <metadata.json> --out-dir <new-directory>
bun run llang inspect <capability.json> --out-dir <new-directory> --json
```

package成功時のverification/acceptanceはnot-run。これを検証合格と解釈しない。inspectは `program.inspection.ts` と `inspection.json` を出力する。reportはformat=`llang-capability-inspection`、version=1、typescript.source/projectionHash/sourceHash/programHash、artifacts.artifactHash、packageHashを持つ。元のsemanticEquivalence=not-checkedを保持する。

`llang develop` は使わない。SAAA providerがsourceを生成し、固定CLIのpackageでbuildする。新しいmodule-value等のprofileがL-Lang側に存在しても今回取り込まない。旧v1向け `capability verify` も呼ばない。

開発checkoutをアプリ実行時の依存にしない。新規 `scripts/llang/build-generation-kit.ts` でCLIと依存資産を自己完結した管理kitへコピー/bundleし、entrypoint・file一覧・hash・L-Lang HEAD/dirty情報・Bun版をmanifestへ保存する。パスに空白を含む別directoryへ移してpackage/inspectが動く試験を必須とする。expectedDigestは管理者設定値と照合し、起動時の計算結果を自動信頼しない。

新規 `SAAA_LLANG_GENERATION_CONFIG`（絶対path）のJSON:

```json
{"formatVersion":1,"enabled":true,"bunPath":"/absolute/bun","kitRoot":"/absolute/kit","expectedKitDigest":"<64hex>","requestsPath":"/absolute/requests.json"}
```

設定上限64KiB、未知field拒否、enabled省略不可。無効・未設定なら生成/inspectionコマンドだけunavailableとし会話・MCPを維持する。kitのtoken/ネットワーク設定は禁止。kit未完成の場合はL-Lang依存未達としてC01を止め、後述fake generatorの独立試験のみ進める。

### 12.2 ホスト登録要求・モデル応答

requestsPathは最大1MiB、formatVersion=1、entries最大32。各entryの必須項目:

| field | 型・制約 |
| --- | --- |
| id | `[a-z0-9][a-z0-9_-]{0,63}`、一意 |
| capabilityId | ホスト管理の論理ID。source/request/metadataのidへ同じ値を設定 |
| purpose | UTF-8 1〜4096bytes |
| fields | 一意なboolean field名1〜8。ASCII identifier、順序固定 |
| requestPath / suitePath / metadataPath / acceptanceId | ホストの固定ファイル・既存acceptance参照。絶対path、実在、読込時にhash固定 |
| scope | `{"kind":"user"}` または `{"kind":"project","id":"実在ID"}` |
| allowCreate / allowUpdate / autoActivate / grantOnCreate | 必須boolean。省略時にtrueを補わない |

request/suiteは既存candidate fixtureのv2構造に従う。request契約とfieldsの順序・名前を比較する。suiteと独立acceptanceを分け、どちらもモデルへ書き換えさせない。requestの本文と契約はモデルへ渡すが、独立acceptanceの正解表は渡さない。

モデルに許す応答は厳密JSON `{"formatVersion":1,"source":{...L-Lang Source v1...}}` のみ。Markdown fence除去やJS抽出で救済しない。source.language/version/id/profile/contractを固定要求と比較し、不一致はgeneration-contract-mismatch。source.bodyだけが自由生成の中心となる。LLMが返したsuite/metadata/path/commandは未知fieldとして拒否する。sourceをJSONとして保存すれば有効JSONCでもある。

sourceは既存validator/CLIに検証させる。任意JS/evalを許す変換は追加しない。package用metadata.releaseはjob UUIDからホストが生成し、idはcapabilityId固定。他metadataは登録ファイルからコピーする。request/suite/metadataのbyteをjob workspaceへコピーして使い、生成中の元ファイル変更を読まない。

### 12.3 型と関数

Rustのpublic内部型は `generation/contracts.rs` に集約し、serdeはcamelCase・deny_unknown_fields。principal/project/run/messageはモデル入力から受けない。

```rust
struct GenerationContext {
    principal_id: String,
    conversation_id: String,
    run_id: String,
    input_message_id: String,
    project_id: Option<String>,
}
struct GenerateInput {
    request_id: String,
    base_revision_id: Option<String>,
}
struct GenerationReceipt {
    job_id: String,
    status: GenerationStatus,
    revision_id: Option<String>,
    error_code: Option<GenerationErrorCode>,
}
// Arcを所有する管理taskとして実行し、caller dropでは中途放棄しない。
async fn generate(self: Arc<Self>, context: GenerationContext,
    input: GenerateInput, cancellation: RunCancellation) -> GenerationReceipt;
async fn inspect_execution(&self, context: InspectionContext,
    call_id: &str, cancellation: RunCancellation) -> Result<InspectionReceipt, InspectionError>;
```

InspectionContextはprincipal/project/conversationをホストから持ち、InspectionReceiptはinspection_id、revision_id、source_hash、program_hash、artifact_hash、projection_hash、typescript_text、report_json。pathをクライアントへ渡して任意ファイルを開かせない。report/body合計1MiBをIPC側でも検査する。

GenerationStatusは12.4のstatus CHECKと一対一のenumとする。未知状態をdeserialize時に拒否する。InspectionReceiptのreport_json/comparison_jsonはversionを検証したserde_json::Valueとし、生文字列を二重parseしない。

error_codeは `unavailable`, `unsupported-request`, `not-authorized`, `invalid-input`, `generation-contract-mismatch`, `model-error`, `budget-exceeded`, `build-error`, `acceptance-failed`, `conflict`, `cancelled`, `interrupted`, `integrity`, `storage` に固定。生stderr/provider本文をユーザーへ返さない。inspectionは加えて `not-generated`, `artifact-missing` を持つ。エラーに自動retryを示す値を付けない。

providerは現 `providers/openai_compatible/structured.rs::complete_tool_less` と同じ設定選択・credential取得を再利用するが、生成専用optionsを追加してmax output 4096と外側deadlineを適用する。既存訂正抽出の上限を変更しない。未対応providerはunavailable、他providerへ無断fallbackなし。

### 12.4 DDLと所有者記録

以下を既存migration transaction内で追加する。既存tableをdropしない。時刻はunix milliseconds INTEGERに統一する。

```sql
CREATE TABLE generated_capability_generation_jobs (
 id TEXT PRIMARY KEY,
 principal_id TEXT NOT NULL,
 conversation_id TEXT NOT NULL REFERENCES conversations(id),
 run_id TEXT NOT NULL,
 input_message_id TEXT NOT NULL,
 project_id TEXT,
 request_id TEXT NOT NULL,
 request_snapshot_json TEXT NOT NULL CHECK(json_valid(request_snapshot_json)),
 request_digest TEXT NOT NULL CHECK(length(request_digest)=64),
 capability_id TEXT NOT NULL,
 base_revision_id TEXT REFERENCES generated_capability_revisions(id),
 expected_epoch INTEGER NOT NULL CHECK(expected_epoch>=0),
 revision_id TEXT REFERENCES generated_capability_revisions(id),
 status TEXT NOT NULL CHECK(status IN ('requested','generating','building','importing',
 'verifying','awaiting_activation','active','failed','cancelled','conflict','interrupted')),
 error_code TEXT,
 usage_json TEXT CHECK(usage_json IS NULL OR json_valid(usage_json)),
 created_at INTEGER NOT NULL,
 completed_at INTEGER,
 UNIQUE(principal_id,run_id,input_message_id)
);
CREATE UNIQUE INDEX generated_generation_one_active_job
 ON generated_capability_generation_jobs(capability_id)
 WHERE status IN ('requested','generating','building','importing','verifying','awaiting_activation');
CREATE TABLE generated_capability_call_owners (
 call_id TEXT PRIMARY KEY REFERENCES generated_capability_calls(id),
 principal_id TEXT NOT NULL,
 conversation_id TEXT NOT NULL REFERENCES conversations(id),
 project_id TEXT,
 run_id TEXT NOT NULL
);
CREATE TABLE generated_capability_inspections (
 id TEXT PRIMARY KEY,
 revision_id TEXT NOT NULL REFERENCES generated_capability_revisions(id),
 inspector_digest TEXT NOT NULL CHECK(length(inspector_digest)=64),
 package_hash TEXT NOT NULL CHECK(length(package_hash)=64),
 source_hash TEXT NOT NULL CHECK(length(source_hash)=64),
 program_hash TEXT NOT NULL CHECK(length(program_hash)=64),
 artifact_hash TEXT NOT NULL CHECK(length(artifact_hash)=64),
 projection_hash TEXT NOT NULL CHECK(length(projection_hash)=64),
 relative_directory TEXT NOT NULL UNIQUE,
 comparison_json TEXT NOT NULL CHECK(json_valid(comparison_json)),
 created_at INTEGER NOT NULL,
 UNIQUE(revision_id,inspector_digest)
);
```

inspection tableは完全成功の記録だけを持ち、running/failed行は作らない。エラーは呼出元へ返す。初版のinspection比較失敗ではTS/reportを成功成果物として公開しない。旧3章のstatus列指定はこのDDLで置き換える。

tool_selectionのLlangBackendは `BackendRequest.call_id` をそのまま `InvokeRequest.call_id` に渡しているため、新たなID対応表は作らない。同一IDでgenerated_callsへjoinする。BackendRequestにホスト由来のactor contextを追加し、CapabilityServiceがcall行とowners行を同一transactionで作成する。M2Aの直接経路もactorを渡す。originはconversation/mcpを正しく伝播させる。

既存のowner不明callはmigrationで現在のユーザーへ帰属させない。inspectionはnot-authorizedで拒否する。通常利用開始後のcallから検査可能とし、管理者向け履歴回収は今回作らない。InspectionContextのprincipalとownersが一致し、project付き記録はcontext.projectも一致する必要がある。実行grantの撤回・suspendだけでは所有済履歴の閲覧権を消さない。

### 12.5 1 transactionでの公開

新規 `generated_capabilities/publication_sync.rs::activate_and_publish(connection, ...)` を作る。CapabilityServiceはhash/runtime/受付状態の事前検査を行い、writer transaction内で既存guardの最終条件・passed check・expected_epochを再確認する。guards.rsのtransaction内関数を抽出して再利用し、検証を複製して弱めない。

transaction順は、①元revision/epoch再照合、②既存revisionのactive切替、③tool_selection source/catalog/revision/usage/FTS登録、④許可されたgrant差分、⑤catalog/acl epoch更新、⑥job active/completed_at更新。どこで失敗しても全部rollback。DB lock保持中にモデル・process・embeddingを呼ばない。

source_idは `llang-generated:<principal_id>`、tool_idは `llgt_` + SHA256(canonical JSON `[source_id,capability_id]`)。tool_selection revision_idは `llgr_` + SHA256(canonical JSON `[tool_id,generated_revision_id,contract_hash,catalog_epoch]`)。同じgenerated版へ戻してもbinding内epochを更新した新しい公開revisionにする。bindingは既存LlangBindingの全fieldを埋める。usageはpurpose/用法とboolean入力契約から作り、モデルに権限説明を生成させない。

変更1batchにつきcatalog_epochは1回、grant変更があればacl_epochも1回。1件ごとepochを増やす旧register_revisionのラッパーをそのままループしない。embeddingはcommit後に生成し、失敗時は既存BM25縮退で利用可能とする。

suspend/retireは同transactionでtoolを非公開化しcatalog_epoch更新。旧revision復帰も同じ公開関数を使う。呼出元にwriterを公開しない。既存内部APIから状態変更する場合もこの同期を通す。新しいsourceの作成だけではgrantを作らない。

初回grantは登録要求のgrantOnCreate=trueかつ要求scopeがcontextと一致する場合のみ。user scopeはprincipal_id、project scopeは登録project_id固定。更新時は既存grantを保持し追加しない。他人のcapabilityを同IDで更新しないよう、source/principalの対応を先に検査する。autoActivate=falseは候補検証完了時点でawaiting_activationを返し、内部管理APIによる公開のみ可能。再起動時は自動公開しない。

### 12.6 jobの実行・復旧

開始時に要求設定をimmutable snapshot化しDBへ保存する。重複 `(principal,run,message)` は既存receiptを返し、モデルcallを増やさない。同capabilityに別jobが進行中ならconflict。初回のexpected_epochは0、更新は開始時の実値とする。

state更新は `UPDATE ... WHERE id=? AND status=?` のcompare-and-swapで1行を要求する。順序は5章どおり。キャンセルは任意の非終端からcancelled。エラーはfailed、epoch不一致はconflict。commit済activeを遅れて来た取消でcancelledへ変えない。

1profile同時生成1件、待機queueなし。job管理taskがpermit/process/DB終端を所有する。呼出元dropは取消を伝達し終端までtaskを維持する。model+package120秒、続く既存import/verifyは各既存timeoutを保持しjob全体180秒で打ち切る。検査15秒は別操作の上限。

再起動時のrequested/generating/building/importing/verifyingはinterruptedへ。awaiting_activationは検証済み候補として維持するが、新しい管理操作でhash/runtime/epochを再検査しない限り公開しない。temp directoryは `<data>/generated-generation/<job_id>/`、保存packageは既存PackageStore。失敗workspaceの保持上限は32件/64MiB、古い完了jobから削除しhash/診断記録は残す。任意pathをcleanupしない。

inspectionのdirectoryは `<data>/generated-inspections/<inspection_id>/`。同revision/digestはmutexでsingle-flight、完成fileをrename後DBへ登録する。DB失敗で残るorphanは起動時削除。DB行に対するfile欠落/改変は再生成で隠さず明示エラー。再生成は管理操作で別inspector digestを採用する時だけとする。

### 12.7 会話入口・表示を固定する

新規 `runtime/capability_commands.rs` で、ユーザーの保存済み入力本文全体が以下と一致する場合だけ処理する。

```text
/capability generate <registered-request-id>
/capability update <registered-request-id> <base-revision-id>
/capability inspect <call-id>
```

前後空白はtrimするが改行・追加引数・引用内のcommandは受け付けない。通常会話モデルやtool出力からcommandを起動しない。`runtime/turns.rs` のユーザー入力永続化後、tool_selection.begin_turnと最初のprovider呼出しより前に分岐する。通常のrun取消・terminal event・assistant message永続化経路を使い、別runを作らない。

generate/updateはreceiptと次のsearch intentを表示する。active後の利用は通常会話の3入口に戻す。要求を受けた直後に副作用toolを自動実行しない。初版は自然文からの無条件生成を提供しないことをヘルプに明記する。

inspectは既存assistant messageとしてhash/report要約とescaped TypeScriptコードブロックを永続化・表示する。専用UI・任意file-open IPCは追加しない。大型出力は既存artifact機構へ載せる代わりに、この版では表示上限64KiBを設け、超過時はsummaryとartifact-too-largeを返す。管理領域の全成果物は保持する。コード内のbacktickを考慮して適切な長さのfenceを選ぶ。これはv1の任意artifact/IPC接続の選択肢を置き換える。

### 12.8 固定fixtureと反例

入力順 `[enabled,suspended]`、列挙順 `00,01,10,11` を固定する。要求Aは `enabled && !suspended`、期待値 `[false,false,true,false]`。変更要求Bは `enabled`、期待値 `[false,false,true,true]`。fake generatorは要求からsource.bodyを組み立て、事前packageを選択しない。A/Bのrequest・suite・independent acceptanceは別々に固定する。

最低限、①A生成成功→通常検索→call→inspect、②B生成成功→A callのTS不変、③全trueを返す不正sourceでacceptance失敗・A維持、④途中epoch変更でconflict・grant増加0、⑤別principal inspection拒否、⑥改変TSでintegrity、⑦caller abortでprocess回収/次job開始可、⑧公開transactionの各段階rollback、⑨同入力message二重到着でmodel call1、⑩独立正解表をmodel promptへ含めない、を試験化する。

live試験では固定A/Bとは別に人が事前作成した新規要求と全入力の独立正解表を使用する。正解表は生成後に変更しない。モデル生成が失敗した場合は実証未達と報告し、fakeの結果で代替しない。

## 13. 小カードへの分解とレビュー順

7章Tカードは集計用に残し、実装は以下のCカードを順に進める。各カードで指定試験が通るまで依存カードへ進まない。

| ID / 対応 | 変更ファイル | 実装と必須試験 |
| --- | --- | --- |
| C01 / T00 | scripts/llang/build-generation-kit.ts、依存記録 | 実在package/inspectを固定し移動可能kitを生成。hash改変・空白path・checkoutなし試験 |
| C02 / T01 | generation/config.rs、contracts.rs | 設定/要求/モデル応答の型。未知field・不一致id・scope・64KiB超を拒否 |
| C03 / T02 | generated_capabilities/schema.rs、generation/repository.rs | 12.4 DDL、CAS、job idempotency。upgrade/FK/同capability競合 |
| C04 / T02 | contracts.rs、service.rs、tools.rs、backends/llang.rs | actor context伝播、call+owner同時保存。M2A/MCP/通常会話別の所有者試験 |
| C05 / T03 | inspection/contracts.rs、service.rs、repository.rs | callからrevision固定、CLI report全hash照合。現activeへの置換を拒否 |
| C06 / T04 | inspection/comparison.rs、固定比較worker | 最大256入力、projection/Wasm一致・意図的不一致・timeout |
| C07 / T05 | guards.rs、publication_sync.rs、catalog.rs | transaction内guardを抽出、両台帳/grant/jobをatomic更新。各境界rollback |
| C08 / T05 | lifecycle.rs、publication_sync.rs | suspend/retire/復帰、binding epoch更新、旧ref拒否、過去TS維持 |
| C09 / T06 | generation/generator.rs、builder.rs | tool-less providerと固定package CLI。fake A/B、JSON不正、secret非継承 |
| C10 / T07 | generation/service.rs、recovery.rs | 状態遷移・予算・取消・import/verify/publication接続。二重message/再起動 |
| C11 / T08 | runtime/capability_commands.rs、turns.rs | 3command厳密parse、保存済みuserのみ、通常conversation終端経路 |
| C12 / T04,T08 | capability_commands表示処理・試験 | TS fence escape、64KiB表示上限、hash/report、他人のcall拒否 |
| C13 / T08 | generated_capabilities/tests/generation_flow.rs | A→実行→B→A検査→停止→復帰を会話とMCPで通す |
| C14 / T09 | live試験harness、検証報告 | 固定kit/実モデル/新規要求＋変更要求。usageと未達を明示 |

C13までの決定的試験に外部API credentialを要求しない。C14は明示live laneとし、model/kit版・費用取得可否・生成物hashを残す。C14が動かせなくても検証済みCカードの成果は残し、全体完了とはしない。

R1では、①生成開始の権限、②モデルが触れるfield、③検証前公開の不存在、④DB transaction境界、⑤過去版inspectionの所有者、⑥失敗時process/DB/fileの終端を追う。各指摘は失敗fixtureを先に追加してから修正する。R2で既存M1/D0〜D5とC01〜C14の結果表を提出する。
