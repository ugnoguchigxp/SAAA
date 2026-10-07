# 機能別の検証画面と変更範囲に応じた検証

作成日: 2026年10月7日 JST

追記: ユーザーの完成条件はフルセットHTTP起動、全業務単体テストのTauri非依存実行、接続システムON/OFF、診断単独起動である。以下の機能限定方針は[改訂計画](http-runtime-and-test-profiles.md)で見直した。画像labを最終成果とはしない。本書の調査記録は作成時点の情報として残す。

状態: 調査に基づく設計提案。製品実装・実Provider呼出し・コミット・プッシュは行っていない。以下の新しいcrate名、コマンド名、HTTP経路は提案であり、現在は使えない。

実装順序・crate名・受入条件は[統合したドメイン分割計画](rust-domain-crate-migration.md)の第14節を使う。並行するメモリー改善との所有者・引渡し条件は同書の第15節に従う。mediaの試用hostへMemory・Context全体を取り込まず、Memory crate化をメモリー改善の前提にしない。ContextStillによるSQLite自動取得・vibe memory化は既存前提であり、本提案にも追加しない。

## 推奨する進め方

既存React部品を使う小さな検証用入口を設け、画像生成からTauriに依存しないRust処理を取り出す。Tauri版とローカルHTTPサーバー版が同じ処理を呼ぶ。アバターの描画はブラウザだけで先行して試せるようにする。検証対象の選択は既存の`verify`に追加し、人やLLMが毎回packageを列挙する運用にしない。

最初から全バックエンドをサーバー化しない。crate分割は「この機能を変更してもデスクトップ全体をコンパイルせず試せる」という境界が成立する分だけ行う。CLIは起動入口として使い、生成内容の入力、進捗、取消、画像・音楽の確認は既存画面で行う。

## 現コードで確認したこと

調査対象は既存checkout、branch `feat/self-diagnosis-v2`、HEAD `cced8b8177b615b50167841fe760a823c0ecbf8b`と当日の作業ツリー。Codexの正規`list_projects`でSAAAのprojectId `7373d28d-7ef9-4a7d-8117-86f0363a4e8b`とこのパスの対応を確認した。並行した未コミット・未追跡のRust変更があり、調査中にも増えているため、固定commitだけの評価ではない。

[AGENTS.md](../../AGENTS.md)を適用した。リポジトリ内の`.agents/skills`は存在せず、ユーザー共通の`.agents/skills/system-estimation`は工数・費用見積用で本調査には適用しない。過去の設計は[用途別クラウドAPI切り替え計画](purpose-based-cloud-api-switching.md)と照合したが、現状の判断は実コードを優先した。

| 対象 | 根拠と設計への含意 |
| --- | --- |
| Cargo構成 | ルートCargo.tomlと`[workspace]`はなく、6個の独立manifest・lockfileがある。`src-tauri`は4個の`crates/*`と`services/reasoning-mcp`へpath依存。reasoning-mcpはlarm-sessionとreasoning-contractに依存する。[desktop manifest](../../src-tauri/Cargo.toml)、[service manifest](../../services/reasoning-mcp/Cargo.toml)。共有target-dirはすでに[.cargo/config.toml](../../.cargo/config.toml)で設定済み。 |
| コンパイル境界 | desktopのTauri依存はoptionalではない。`custom-protocol`、`offline-contracts`、各harness featureはドメインを切り出す機構ではない。[lib.rs](../../src-tauri/src/lib.rs)が多数の領域を一つのrlibに含める。`--lib media_generation::`は実行するテストを絞るだけで、独立コンパイルにはならない。 |
| 起動とnative依存 | [lib.rs](../../src-tauri/src/lib.rs)のsetupはDB、Memory、Situation、MCP、queue、到達性監視等を初期化する。[AppState](../../src-tauri/src/app_state.rs)もこれらを保持する。[build.rs](../../src-tauri/build.rs)にはmacOS VPIOのCコンパイル、Codex配置、role-routing sidecarのBunコンパイル、tauri-buildがある。これらはbuild scriptが再実行されるときの費用であり、毎回すべてが再実行されるとは限らない。 |
| 画像・音楽 | [generation.rs](../../src-tauri/src/media_generation/generation.rs)は用途Routeを解決し、LARM MediaClientまたはReplicateを呼ぶ。State、Channel、SQLite、資格情報への依存が同居する。[mod.rs](../../src-tauri/src/media_generation/mod.rs)はプロセス全体のRUNSを保持し、取消と同時実行を管理する。古い計画の「LARM直接依存のみ」はすでに現状と異なる。 |
| 既存の再利用単位 | [larm-session/media.rs](../../crates/larm-session/src/media.rs)はTauriを使わず、discover/generate/成果物取得を扱う。[service_registry](../../src-tauri/src/providers/service_registry.rs)には型、検証、Route解決があるが、同じmoduleにIPC、AppState変換、probeもある。pureと書かれたコメントだけで独立済みと見なせない。 |
| frontend | [MediaGenerationPanel](../../src/features/media/MediaGenerationPanel.tsx)は`api`を注入でき、画像・音楽の既存[単体試用画面](../../src/features/providerUnitTest/ProviderUnitTestPage.tsx)でも使われる。[mediaApi.ts](../../src/features/media/mediaApi.ts)にinvoke、進捗Channel、binary応答が集まる。一方[App.tsx](../../src/App.tsx)は全体snapshotを要求し、[runtime.ts](../../src/lib/runtime.ts)等もTauriを直接呼ぶ。Viteだけ起動して全Appを開く方法では足りない。 |
| アバター | [LightAvatarBackground](../../src/features/chat/avatar/LightAvatarBackground.tsx)はcueを受ける描画部品。[useAvatarDecision](../../src/features/chat/avatar/useAvatarDecision.ts)はTauri event購読。Rustの[speech_expression.rs](../../src-tauri/src/runtime/conversation_check/speech_expression.rs)がAppHandle経由で音声開始・終了を通知する。描画、Laya推論、native再生同期は別々の検証対象。 |
| 既存の軽い入口 | [generative-ui-preview.tsx](../../scripts/generative-ui-preview.tsx)と[専用Vite設定](../../scripts/generative-ui-vite.config.ts)に、製品入口を使わず既存部品を表示する前例がある。[reasoning-mcp/main.rs](../../services/reasoning-mcp/src/main.rs)には独立Rust＋Axumのloopbackサーバーがある。ただし媒体生成の入口ではない。 |

### 待ち時間の根拠と未計測部分

[verify-plan.ts](../../scripts/verify-plan.ts)を読み取り専用で評価した結果、normalは25 step、advanceは42 step、fullは44 step。Rustは6 packageそれぞれにfmt、clippy、check、必要な段階ではbuild/testを計画する。各packageのdev-dependencyやfeatureが違うため、共有target-dirだけで全成果物が共用されるとは限らない。[verify.ts](../../scripts/verify.ts)はstepを直列実行し、成功ログを消すため、現在は成功時の段階別時間が残らない。

さらに[frontend-tests.ts](../../scripts/frontend-tests.ts)はmodule mockの混線を防ぐためファイルごとに別Bunプロセスを使う。標準gateの`--fail-fast`では並列数1となる。単純な一括実行への変更は試験の意味を変える。

[既存検証記録](../../logs/commit-verification-20261007.json)はadvanceがdesktop lintで停止し、以後未実行と記録する。[そのログ](../../logs/commit-verification-rust-lint-20261007.log)のCargo部分に31.91秒の表示があるが、これは一回の観測で全体時間でも比較基準でもない。同記録にはClippy追加warning key 231、module-size 63件、spec 109件の失敗がある。今回再実行していないため、現在も同数とは断定しない。

[共有ロック](../../scripts/verification-lock.ts)はGit common-dir単位で、[start](../../package.json)のTauri devは終了まで保持する。試用しながら別検証を走らせる場合の待ちも区別して測る。rlibのみの生成とdev/testの`debug=1`はすでに導入済みで、今回の新しい改善として数えない。起動時間、増分build、リンク、ディスク増加は未計測であり、短縮率は未算出。

## 共有する境界

依存方向は次の形にする。名称は仮称で、初期は共通crateを一つに留める。

```text
既存MediaGenerationPanel ─ MediaApi ─┬─ Tauri adapter ─ desktop command ─┐
                                    └─ HTTP adapter ─ feature-lab ──────┤
                                                                    ▼
                                                  saaa-feature-runtime
                                              media / registry / repositories
                                                  ▼                 ▼
                                             larm-session     Replicate adapter
```

`saaa-feature-runtime`と`services/feature-lab`はTauri、saaa_lib、native build scriptに依存しない。desktopの別binからsaaa_libを呼ぶ形ではこの条件を満たさない。画像と音楽は台帳・成果物処理を共有するため、最初から別crateにしない。既存larm-sessionを画像専用crateへさらに分割するかは、その実測が律速になってから決める。

| 責務 | 共通処理とhost固有処理の境界 |
| --- | --- |
| 設定・選択 | registry型、validate、resolve、fingerprint、実行中の有効性確認、既存legacyからの導出に必要な純粋処理を共通化する。現状は[providers.registry/default](../../src-tauri/src/persistence/service_registry_store.rs)とlegacyのoverlayが正本であり、過去計画の3 document案へ戻さない。実行開始時のRoute固定、expectedRevision、保存の原子性を維持する。HTTP版が独自の設定JSONやProvider選択規則を持たない。 |
| 資格情報 | [credentials.rs](../../src-tauri/src/credentials.rs)は現在SQLiteのcredential_secretsとグローバルOnceLockを使う。OS Keychainとは仮定しない。共通CredentialStoreをinstance単位に渡し、既存backendを移す。[LARM credential](../../src-tauri/src/providers/dynamic_lan/credential.rs)の環境変数／ユーザーファイル読出し・競合検出も共通化する。ブラウザへ秘密値を返さない。検証起動で暗黙に通常ユーザーの資格情報をロードしない。 |
| 状態・取消 | RUNSはMediaService instanceの中へ移し、runId、取消token、期限、同時実行制限を両hostで共有する。[ledger](../../src-tauri/src/media_generation/ledger.rs)、[recovery](../../src-tauri/src/media_generation/recovery.rs)の予約・復旧・成果物採用を一つの実装にする。取消受付とProvider側停止確認、結果不明、mayHaveGeneratedを区別する。HTTP切断を取消完了や再送許可にしない。 |
| 保存 | SQL、transaction内の検証、状態遷移を共通repositoryへ移し、desktopは既存Writer経由で呼ぶ。labも同じrepositoryと対象tableの初期化を使い、隔離DBを所有する。成果物は既存上限を守り、runId/indexで読む。任意ファイルパスをHTTPへ公開しない。 |
| 進捗・stream | 共通EventSinkと結果型を定義し、Tauri Channel/eventとHTTP streamは配送だけを担当する。mediaContractsの検証とrunId照合を共用し、媒体の進捗streamと将来のLLM token/audio streamを混同しない。UIは台帳の状態を再取得でき、frontendを第二の正式台帳にしない。 |
| native | Window、AppHandle、WebView、native音声、OS権限はdesktop adapterへ残す。labで未対応な機能は未対応と表示し、成功するダミーで全Appの初期化を通さない。 |

保存層の抽出は最大の注意点である。[SqliteWriter](../../src-tauri/src/persistence/sqlite/writer.rs)はMemoryのforget journal、全schema初期化、移行前backupに依存するため、そのまま新crateへ移すと他領域まで引き込む。まず既存transactionの内側で動くregistry/media SQLと対象schema定義を取り出し、hostのWriter所有・migration・journalは既存契約を維持する。lab側の小さなDB ownerも同じ所有lockの部品を使う。汎用DB traitの追加だけでこの作業が完了したとは扱わない。

初期labは空の隔離DBを使い、通常DBをWriterで開かない。既存設定を使う場合も明示的な読取snapshotから共通loaderで導出した対象設定だけを取り込み、書戻さない。巨大な通常DBコピーを標準手順にしない。最小実証の認証は明示投入したlab専用値とする。既存資格情報の読取参照を追加する場合は、[validate_active](../../src-tauri/src/providers/service_registry/active.rs)が同じDBのcredential_secretsを確認している点も合わせて変更する必要がある。共通CredentialStoreと有効性確認の整合を保ち、HTTP hostだけSQLのsecret存在確認を飛ばさない。migration自体の試験が必要な段階では別の小さなfixture DBを使う。二つのhostが通常DBへ同時書込みする運用は採用しない。

### ブラウザとサーバーの入口

仮称`bun run lab`を一度起動し、画像・音楽・アバターのタブを開く。起動時はmockで、実Providerへの切替と生成操作は画面から行う。生成画面、エラー、履歴、成果物表示は既存部品を使い、全Appのsnapshotや会話キューを起動しない。

画像の最初のHTTP契約は、送信・取消・履歴・再照合・成果物読出しの5操作に限定する。`POST /media/runs`の応答をfetchで読む進捗＋最終結果streamとし、UI adapterは既存の`generateMedia(input, onProgress): Promise<MediaOutput>`に合わせる。HTTP受付直後に同期処理をrequestの寿命へ丸ごと結び付けず、runの所有者は共通MediaServiceに置く。切断後は履歴／再照合で復帰し、同じrunIdを自動POSTし直さない。streamにはrunIdと順序情報を持たせ、遅延・重複・終端後イベントを除外する。有限queueと上限を置き、遅いconsumerが生成や取消を止めない。

loopback bind、Host/Origin制限、起動ごとのsession認証、body/成果物サイズ制限を設ける。Viteからは許可した同一origin proxyを使い、任意originへ開放しない。tokenをURLやログへ出さず、Provider keyをVite環境変数へ置かない。Tauriの任意invokeを名前だけでHTTPへ転送する汎用bridgeは作らない。

新launcherのコンパイルは既存共有ロックを取得し、完了後に解放して既存binaryを実行する。[verification-build.ts](../../scripts/verification-build.ts)にbuild後に解放する前例がある。再buildは再びロックを取る。現行`start`のロックを迂回したり、別target-dirで同時Cargoを走らせたりしない。

## 手動package指定を不要にする検証入口

現行の`verify`とnormal/advance/fullの意味は保持し、仮称`bun run verify affected`を日常の静的確認、`bun run verify affected --level advance`を影響範囲のbuild/test確認にする。`--explain`は選んだ対象・理由・未対象・全体へ広げた理由を表示する読み取り専用の計画確認とする。実行成功は従来どおり一つの`OK`、詳細な対象と時間は別の機械可読reportへ残す。

選択器は`verify-plan.ts`の前段に置き、各検証の実行・ロック・失敗時停止は`verify.ts`を使う。現在の`--package`は一つだけで、TypeScriptと同時指定するとエラーになるため、複数対象は新しいplan内部で合成する。CLI引数を単に連結する実装にはしない。

1. **差分を集める。** 通常はHEADに対するindexと作業ツリー両方の差分、および非ignoreの未追跡ファイルを合併する。renameは旧・新パス、削除は旧所有者も対象にする。CIやbranch全体では明示したbaseとのmerge-base以降のcommit差分も合併する。baseが不明なら対象を空にしない。ignoreされた生成入力は入力manifestで別途宣言し、秘密値をreportへ記録しない。
2. **入力と利用側を解く。** 現在の6 manifestのpath依存を読み、将来は検証入口内部からoffline/lockedのCargo metadataでtarget・feature・build/dev依存も補う。frontendはTypeScript import graphだけに頼らず、動的import、CSS/asset、生成型、Rust IPC/event、SQL、context入力、テストfixtureを機能の入力manifestに記載する。変更前後の依存graphを合併し、削除した依存関係も見落とさない。
3. **段階に合わせて広げる。** 変更所有者と逆依存の利用側を選ぶ。依存先はCargoがcompileするが、compileと依存先自身のテストは別なので、関連contract suiteを入力manifestで指定する。共有registryやlarm-session変更はmediaだけに絞らず、reasoning-mcp、desktopの音声・会話等の利用側へ及ぶ。Rust module単位の変更でもdesktop内ならコンパイル境界はdesktop全体のまま。
4. **不明なら広く実行する。** 未登録path、解析不能、graph不整合、Cargo.lock、package/bun lock、共通設定、toolchain、build script、verify自体の変更は安全側へ広げる。静的確認なら全体normal、readinessなら影響に応じ全体advance。version更新・大規模変更はfullと該当E2Eを計画する。実サービスや実マイク試験は自動で費用・権限を使う検証に混ぜない。
5. **同じ入力への結果か確認する。** ロック取得後にHEAD・index・入力内容・未追跡集合・選択規則・toolchainをfingerprint化して計画を再確定する。開始後の変更を監視し、各段階後と終了時にも再確認する。途中変更があればstaleとして成功扱いせず再計画する。検証ロックはeditorや別agentの編集を止めない。開始・終了hashだけでは途中に変更して元へ戻すケースを検出できず、watcherの欠落もあり得るため、immutableな入力の証明にはしない。並行編集が続く場合は結果を「観測した範囲の参考値」とし、編集停止を調整した安定入力での再実行なしにreadinessを認定しない。再計画が続く場合は有限回でstaleを返し、編集や設定を巻き戻さない。

TypeScriptのlint/typecheckは当初、言語全体を維持する。選択による節約はRust packageとfrontendテスト群から始める。テストは選択後もファイルごとのprocess分離を維持し、0件実行を成功扱いしない。generated-contextとmodule-sizeは既存のproject-wide実行を明示追加する。現在のRustだけのscoped planではこれらが省略されるため、scoped passから全体passを推論しない。qualityも現行scoped advanceには自動追加されず、IPCと合わせて境界変更の計画へ明示する。

初期は検証結果のキャッシュ省略を実装せず、選択の正しさを先に確認する。各stageの同じcommand/feature/target/profileは一計画内で重複除去できるが、clippy、check、build、testを「似ている」という理由で削らない。将来キャッシュする場合もcommit SHAだけではdirty/未追跡変更を表せず、完全な入力とツール版が必要になる。

## 代替案との比較

| 案 | 得られるもの | 限界・判断 |
| --- | --- | --- |
| 既存部品＋mock APIのブラウザpreview | 新Rust境界なしでアバター、画面、進捗・取消表示を試せる。既存preview方式を流用可能 | 実Provider・保存・nativeの証明にはならない。推奨案の最初の段階として採用 |
| 独立した共通Rust＋小さなHTTP host | 既存UIで実Provider、認証、台帳、取消を製品と同じ処理で検証できる | 保存・資格情報・Routeの抽出が必要。画像一機能で費用対効果を測ってから拡張する主案 |
| Tauri内へHTTP bridge、またはsaaa_libを使う別bin | adapter開発の仮の入口になり、全画面の移植を減らせる可能性 | Tauri依存と全体compile/build scriptが残る。今回の「Tauriなし」の完成形にはしない |
| 既存provider E2E／larm-session試験だけを使う | 既存mockサーバーと低水準transport試験をすぐ再利用できる | 画面操作の代わりにはならない。desktopのharnessは全体crate compileが残る。共通処理の回帰試験として併用 |
| Cargo workspace統合・大規模feature化 | manifest/lock管理や依存の統一に役立つ可能性 | workspace化だけではdesktopが小さくならず、feature組合せ・cache無効化も増える。共有targetはすでにある。初期実証の前提にしない |

## 最小実証と段階導入

| 段階 | 範囲と出口 |
| --- | --- |
| 0: 計測と画面 | 既存verifyへ段階別時間とlock待ちの記録を追加する案を先に検証。専用React入口でMediaGenerationPanelにmock API、LightAvatarBackgroundに固定cueを注入し、画面操作を確認する |
| 1: 画像一機能 | 共通crate＋HTTP hostから、既存MediaGenerationPanelを使ってLARM画像生成を実行する。mock providerでは同じRust経路にfake HTTP endpointを注入する。生成、進捗、取消、成果物、履歴再表示、切断・再起動後の不明結果を確認する。desktop adapterも同じ処理へ切替え、labがsaaa_lib/Tauri/native依存を持たないことを確認する |
| 2: 自動選択 | まずshadow運用でaffectedの計画と必要な全体gateの結果を比較する。未追跡・rename/delete・manifest変更・IPC・共通fixture・並行編集のfixtureで漏れを検証してから日常入口へする。選択漏れは規則を直し、baselineを緩めない |
| 3: 音楽・別Provider | 同じmedia境界でReplicateと音楽を追加し、取消未確認、remote job再照合、redirect/MIME拒否を引き継ぐ。Laya実推論とcue event配送は別に追加する。共通化でnative音声経路を変えない |

最小実証でも「providerを直接叩いて画像が出た」だけでは完了にしない。同じrunIdの二重送信防止、送信前取消、Provider送信後の切断、Route無効化、認証失敗、成果物保存失敗を扱い、台帳に採用された結果だけを画面へ返す。LARM画像は同期応答であり、現行recoveryは`synchronous_image_has_no_job`を返す。サーバー再起動で応答を失った画像は不明のまま保持して再送しないことを合格条件とし、復旧成功を必須にしない。jobを持つ音楽・Replicateの再照合は段階3で検証する。既存[Replicate回帰試験](../../src-tauri/src/media_generation/replicate_tests.rs)、[LARM媒体試験](../../crates/larm-session/src/media/tests.rs)、[不明結果等の回帰試験](../../crates/larm-session/src/media/tests/regression.rs)、[UI生成試験](../../tests/media-generation.test.tsx)、[UI復旧試験](../../tests/media-recovery.test.tsx)を移植・拡張の起点にする。

fixtureでの成功は決定的なUI/transport/状態遷移の証拠。実Provider試験は明示した設定・資格情報と小さな依頼で行い、送信数、接続先、結果、外部待ち時間を記録する。ブラウザ試験ではWebGLと音声再生を確認できても、Tauri IPC権限、WebView差、packaging、VPIO/AEC、実スピーカーとの同期は証明できない。native変更時はdesktop smokeと、TTSのみではASR発話が生じず、TTS中の人の声は届く実機条件を別に確認する。

実装後の既存コマンド例（今回未実行）:

```sh
bun run --silent verify test --scope typescript -- tests/media-generation.test.tsx
bun run --silent verify test --scope typescript -- tests/media-recovery.test.tsx
bun run --silent verify test --package crates/larm-session -- --lib media::
bun run --silent verify test --package src-tauri -- --lib media_generation::
bun run --silent verify:advance
```

共通処理・IPC・保存境界の移動をreadinessとする段階では利用側を含む全体advanceが必要になる。変更規模に応じて関連integration/E2E、major変更ではfullを追加する。既知の全体失敗を解消できない間は部分結果と未実行を記録し、build-readyとしない。これはcheckpoint commit/pushの前提条件を追加する提案ではない。

## 効果の測り方とリスク

同じMac、toolchain、profile、入力とProvider条件で、起動から操作可能まで、変更保存から再操作まで、verifyのlock待ち・各stage時間、peak memory、target増分容量を測る。画面だけの変更、media Rust変更、共通registry変更の3条件を分け、warm実行を複数回記録する。外部モデルのcold start/生成待ちをローカルbuild時間から分ける。比較用にcacheを削除したり複数target-dirを作ったりせず、cold条件は自然に必要な初回buildの記録を使う。少数回なら中央値と範囲を示し、根拠のないp95や短縮率を出さない。

採用条件は、媒体変更の試用でdesktop compileが不要になり、同じ失敗・取消・復旧契約が通り、総待ち時間と容量が許容範囲へ改善したと実測できること。affectedはshared変更で広い検証へ戻るため、毎回の短縮は保証しない。抽出による循環依存、migrationの二重化、HTTP版だけの別挙動、input manifestの陳腐化、追加binaryによる容量増加を主要リスクとする。最小画像実証で境界が過大になる場合は、ブラウザmock＋既存crate試験を先に使い、全ドメインの移動へ拡大しない。

## 本提案の確認記録

今回実行したのはコード・既存ログ・manifestの読取りと`verificationPlan`の計画評価、文書の確認のみ。全体build、lint、unit/E2E、実サービス試験は実行していない。新規文書だけを追加し、並行変更を変更・退避していない。

自己レビューを1回行い、(1) LARM同期画像の復旧限界、(2) 外部資格情報参照と同一DB内のsecret存在確認の不整合、(3) 開始・終了hashだけでは並行編集の一貫性を証明できない点を指摘し、本文へ反映した。再レビューの反復は行わない。

文書内のローカルリンク44件の実在、記載した既存検証コマンドのplan生成、normal/advance/fullの25/42/44 step、文書差分の空白を確認した。既存ログの失敗は未再検証。高速化の数値、実Providerとnativeの動作、抽出後の依存graphは今後の実証が必要である。
