# Rustドメイン分割・画像生成lab・affectedの実行用作業票

作成日: 2026年10月7日 JST  
状態: 計画のみ。以下の作業は未着手。今回、実装・検証コマンドの実行・他モデルへの依頼は行っていない。  
設計の正本: [Rustのドメインcrate分割とテスト移行方針](rust-domain-crate-migration.md)。本書は、その方針を一件ずつ実装できる粒度へ分解したもの。

## 1. この作業票の使い方

Grokを含め、担当モデルが会話履歴を知らなくても進められるように、入力、対象、手順、テスト、完了条件を固定する。特定モデルでの実行能力や所要時間を実測したものではない。

- 一度の依頼は作業ID一件。記載した前提IDが完了してから着手する。隣の作業を自動で始めない。
- 一件は一つの振る舞いまたは一つの境界の変更と、その既存テスト移動・追加・検証まで。ファイル数だけで原子的な変更を分断しない。
- 各作業の「対象」は主な変更先。呼出側の機械的なimport・再公開変更は許すが、対象外の仕様修正は混ぜない。想定外の境界変更が必要なら、理由と追加作業票を記録して依存する作業を保留する。
- 既存の実装とテストを読む前に新実装を書かない。型名・エラー文字列・SQL・featureは実コードを正とし、古い計画の記述だけで変更しない。
- 旧本番経路は新経路の接続まで動かす。一時的に未接続の新実装が必要なら作業記録に削除期限のIDを記す。二つの本番実装、二つのRUNS、二つのwriterを同時に正本にしない。
- テスト削除、assert緩和、ignore追加、baseline更新、設定リセットで完了させない。失敗と未実施を残し、単体成功を全体成功と書かない。
- 各段階のアプリ受入が終わるまで、その段階を完了扱いにしない。途中の作業完了とアプリ全体のreadinessは別である。
- 作業記録は`docs/plans/rust-domain-crate-progress.md`を実装開始時に作る。並行編集がある場合、他の変更を上書き・巻戻ししない。

作業票中の`media_generation/`、`providers/`、`persistence/`、`credentials.rs`、AppStateは`src-tauri/src/`配下を指す。M系列の「media」は新package `crates/saaa-media`、「host」は`services/feature-lab`の略。検証時は省略形ではなく表の実ディレクトリを使う。新module・test名は実装予定であり、現在存在するとは限らない。

既存checkout・現在のbranchを使い、作業票ごとのworktree作成や別タスクへの送信は行わない。`initial_instructions`は同じ会話で再実行しない。実装は別途依頼されたときに開始する。今回の「設計のみ・重いbuild/E2Eなし」は、この作業票を作る依頼にも適用する。

## 2. 共通の実装契約

以下は各作業票に繰り返し書かなくても適用される。変更する場合は、実装前に方針書を改訂して根拠を残す。

| 項目 | 固定する契約 |
| --- | --- |
| crate | 最初は`crates/saaa-media`、hostは`services/feature-lab`を予定。Tauri/saaaへの通常・dev・build依存を禁止。Runtime全体を移さない |
| Provider選択 | 現在のregistry・legacy overlay・fingerprint・revision・有効性確認を保持。HTTP専用の選択規則を作らない |
| 保存 | 本番は既存SqliteWriter一つ。共通SQLは借用transaction上で実行し、repository内部で独立commitしない。journal・backupの所有を移さない |
| 資格情報 | mediaへinstanceで注入。desktopは既存保存先、labは隔離DBの明示投入値。通常ユーザーのenv/fileをlabが暗黙読出ししない |
| 実行 | MediaServiceがrunを所有。通信切断と明示取消を区別。結果不明の自動再送を禁止。RUNSとdownload枠をinstance単位へ |
| IPC | 既存5 command、引数名、serde、binary応答、fallbackを保持。HTTP envelopeをTauriへ流用して契約を変更しない |
| 音声 | VPIO、AEC、ducking、TTS中のASR継続は変更しない。preview成功を実機受入の代用にしない |
| verify | 既存共有ロック、共有target、normal/advance/full、成功`OK`、完全診断と即停止を維持。標準チェックを直接Cargo/Bun testで実行しない |

### 検証コードの読み方

下表のV番号は本書内の略記であり、新CLIではない。`<package>`、`<file>`等は実在する対象へ置換する。新package・新testは、該当作業で登録した後にのみ使う。複数packageは別々のverify実行とし、`--package`を連結しない。

| 略記 | 実装時に使う既存入口 |
| --- | --- |
| V-TS | `bun run --silent verify --scope typescript` |
| V-TST(`<file>`) | `bun run --silent verify test --scope typescript -- <file>`。複数ファイルは別実行としprocessを分離 |
| V-R(`<package>`) | `bun run --silent verify --package <package>` |
| V-RA(`<package>`) | `bun run --silent verify advance --package <package>` |
| V-RT(`<package>`, `<filter>`) | `bun run --silent verify test --package <package> -- --lib <filter>` |
| V-MIPC | `bun run --silent verify test --package src-tauri -- --features conversation-queue-e2e --lib media_generation::ipc_tests` |
| V-BOUNDARY | 必要な`bun run --silent verify generated`、`size`、`quality`、`ipc`をそれぞれ実行。省略する項目は理由を記録 |
| V-ALL | `bun run --silent verify:advance` |
| V-FULL | `bun run --silent verify:full`。大規模変更と関連E2E受入に使用 |

Rustのscoped advanceだけではquality/IPC/generated/sizeがすべて含まれるとは限らない。共有型・保存・Provider契約を変更した作業はB00の利用側一覧を加えて検証する。日常は対象と利用側のadvance、コミット前は全体advance、大きな変更時はfullを維持する。保存だけを目的とした明示的checkpoint依頼はAGENTS.mdの例外に従い、未検証を明記する。

失敗したverifyはそこで停止する。修正後に再実行し、後続の未実行チェックを成功扱いしない。重いチェックを実行できない作業は「実装済・受入未完了」と記録する。

## 3. 作業順序

実行順は次のとおり。分岐は依存関係を示すもので、並行エージェントへの依頼ではない。

```text
B00 → B01 → U01 → U02
          → B02 → M01 → M02 → M03 → M04 → M05 → M06 → M07
                                                      ├→ M08 ─┐
                                                      └→ M09 ─┴→ M10 → M11 → M12
M12 → H01 → H02 → H03 → H04 → H05 → G01
B00 → V01 → V02 → V03 → V04 → V05 → V06 → G02
G01 + G02 → G03 → 次の移行対象を一つ選ぶ
```

M08とM09はともにM07の後に行い、M10は両方の完了を要求する。G01までaffectedは検証を省略する根拠にしない。V系列は従来の全体gateを維持したまま実装できるが、日常運用への切替はG02まで行わない。

## 4. 基準とブラウザ画面

### B00 — 現行の境界とテストを一覧にする

- **前提:** なし。AGENTS.md、方針書、作業ツリーを確認。
- **対象:** `src-tauri/src/media_generation/`、`providers/service_registry/`、`credentials.rs`、`persistence/`、`tests/media-*.test.tsx`、`scripts/verify-plan.ts`。変更は作業記録のみ。
- **手順:** HEAD・関連ファイルの状態を記録。5 commandの入出力、台帳状態、SQL対象、資格情報読出し、run所有、各利用側を表にする。既存ケースごとに旧配置・feature・不変条件・移行先・実行gateを記録する。
- **検証:** ソース上のテスト登録を追跡する。実行時の一覧採取はverify経由とし、未実行なら明記。media IPCのfeature条件、Replicate、音楽、larm-session、TS復旧を一覧から落とさない。
- **完了:** 不明な参照先・未登録ケースを含む一覧があり、既存失敗と未検証を区別できる。単にファイル数を数えただけでは未完了。

### B01 — 移動先とAPIを記号単位で確定する

- **前提:** B00。
- **対象:** 作業記録の設計表。製品コードはまだ変更しない。
- **手順:** 移す型・関数ごとに旧所有者、新所有者、引数、返り値、async/同期、transaction所有者、エラー変換、テストIDを書く。MediaServiceの5操作、CredentialStore、EventSink、DB境界、Provider adapterについて具体的なRust宣言案を記す。
- **決定:** registry共用はmedia内の限定moduleを初期案とする。desktopの非media利用側が逆依存や機能依存を持つと判明した場合のみ、狭い`provider-routing`を別crateにする案へ変更し、依存図・後続対象・作業票を先に直す。実装担当が各自で別案を選ばない。
- **検証:** AppState/Tauriの流入、transaction内await、CredentialStoreとactive確認の保存先不一致、schemaコピー、mediaからdesktopへの逆参照がないか表を照合する。HTTPのmethod/path・status・stream終端・認証bootstrapも仕様表にする。
- **完了:** 未決事項が後続作業へ隠れていない。成立しない境界があれば理由を記録し、その依存作業だけ保留。trait追加だけで解決済みとしない。

### B02 — 比較用の記録と移行前の基準を用意する

- **前提:** B01。UIの変更前後も比較する場合はU01より先に行う。
- **対象:** `scripts/verify.ts`・`verification-process.ts`の計測記録、対応テスト、作業記録。製品の処理は変えない。
- **手順:** 既存の静かな成功出力を維持したまま、任意のreportへlock待ち・step時間・command・対象・終了状態を保存できるようにする。まずfake processで検証し、安全な小さい対象で基準を採る。RSS等がまだ取れない場合は未取得とする。
- **検証:** V-TS、既存verify/process/lockテストとreportの新テスト。記録失敗、検証失敗、未到達step、秘密情報除外を確認。移行前のdesktop測定を実施できなければ理由を残す。
- **完了:** 移行前の測定条件と実測値、または明確な未測定が残る。前値なしでG03に速度改善率を算出させない。今回の計画作成中には実行しない。

### U01 — UIの中立な契約をTauri APIから分ける

- **前提:** B01。
- **対象:** `src/features/media/mediaApi.ts`、`mediaContracts.ts`、`MediaGenerationPanel.tsx`と直接の呼出側。
- **手順:** MediaApi型、history schema、表示補助をTauri非依存moduleへ移す。desktop wrapperで現行APIを注入し、既存呼出側を接続する。invoke名・payload・schema判定を変えない。
- **検証:** V-TS、V-TSTで既存`tests/media-generation.test.tsx`と`tests/media-recovery.test.tsx`を別実行。中立moduleのimport経路にTauriがないことを確認。
- **完了:** desktopの既存画面は同じAPIを使い、中立なパネルをTauri初期化なしでimportできる。テストの期待値を緩めない。

### U02 — mockだけの専用previewを作る

- **前提:** U01。
- **対象:** 既存`scripts/generative-ui-preview.tsx`・`generative-ui-vite.config.ts`を参考に、新規`feature-lab-preview.tsx`・専用Vite設定、previewのテスト。
- **手順:** MediaGenerationPanelへ固定mock、LightAvatarBackgroundへ固定cueを渡す。生成成功・進捗・取消・結果不明・履歴を明示的に選べるfixtureを置く。全App・Tauri購読hookを起動しない。
- **検証:** V-TS、V-TSTで新preview契約テスト。ブラウザで表示確認し、Rust起動・実Provider送信がないことを記録。
- **完了:** ここで中断してもUIの単独確認に使える。アバターは描画のみの確認と表示し、音声・推論の成功を表示しない。


## 5. 共通Rust処理の切り出し

### M01 — media crateと既存入出力型を用意する

- **前提:** B02。
- **対象:** 新規`crates/saaa-media/Cargo.toml`・`src/lib.rs`・型module、desktop manifest、mediaの入出力型。
- **手順:** GenerateInput/Output等の中立型を既存serde指定のまま移し、desktopから再公開する。既存larm-session型を再定義しない。必要な依存だけ追加し、依存version更新を混ぜない。
- **検証:** V-RA(`crates/saaa-media`)、V-R(`src-tauri`)、旧新JSON fixtureの一致。verifyの自動package発見に登録されたことを確認。
- **完了:** 型の独立テストがGUIなしで実行され、desktop commandの契約が同じ。空crateのbuild成功だけでは未完了。

### M02 — registryの純粋処理を共用する

- **前提:** M01、B01で所有先確定。
- **対象:** `providers/service_registry/{types,resolve,validate,compatibility,migration}.rs`の必要記号と対応テスト、確定した共通module、旧再公開。
- **手順:** 型・検証・選択・legacy導出を記号表どおり移す。`LocalAvailability::of(AppState)`等の収集はdesktopに残し、値だけ渡す。IPC・probe・Provider起動を移さない。
- **検証:** V-RA(所有crate)、V-R(`src-tauri`)、B00のregistry利用側テスト。既定値・無効route・cloud許可・fingerprint・legacy fixtureの前後一致を確認。
- **完了:** desktopのmedia以外の利用側も同じ実装を参照。選択規則のコピーがなく、依存図がB01と一致。

### M03 — media資格情報をinstanceで渡せるようにする

- **前提:** M02。
- **対象:** `credentials.rs`、`providers/dynamic_lan/credential.rs`、media用CredentialStoreとdesktop adapter。
- **手順:** 明示したwriter/storeで読める実装を作り、media経路からglobal参照を除く。非mediaの既存呼出側は同じbackendへの互換入口を使える。secret存在確認と取得元を合わせる。
- **検証:** 独立した二つのstoreでsecretが混ざらない、未設定・削除・競合が既存エラーになる、秘密値が結果・ログへ出ないケース。V-RA(所有crate)とdesktop資格情報・registryの関連テスト。
- **完了:** mediaにグローバル初期化が不要。保存先・service/account名・既存Provider設定は不変。labの暗黙env/file読出しを既定にしない。

### M04 — 台帳予約を借用transaction上へ移す

- **前提:** M03。
- **対象:** `media_generation/ledger.rs`のinitialize/reserve、共通repository、active確認の必要部分。
- **手順:** schema定義と予約SQLを移し、desktop wrapperが既存writer内でbegin/commitする。route active確認と重複runId拒否を同じtransactionに残す。エラー表現を維持。
- **検証:** 実SQLiteで重複予約、無効route、secret不在、rollback後の再予約。V-RA(`crates/saaa-media`)とdesktop利用側テスト。
- **完了:** repositoryはDBファイルを開かずcommitしない。desktopの本番呼出経路が新SQLを一度だけ使う。

### M05 — 採用と監査を一つのtransactionで共用する

- **前提:** M04。
- **対象:** ledgerのphase/finish、`service_registry/operations.rs`の必要SQL、共通repository、desktop wrapper。
- **手順:** 取消状態確認、route再検証、accepted監査、結果採用を一つのtransactionで実行する形へ移す。unknown/failed/cancelledの判定は元のまま。
- **検証:** 取消先行・採用先行の両順序、active失効、監査INSERT失敗、結果UPDATE失敗で部分commitがないことを実SQLiteで確認。V-RA(media)と対応するdesktop回帰。
- **完了:** 監査だけ／結果だけが保存されない。ネットワークawaitをtransactionへ追加していない。

### M06 — 履歴と成果物の保存・読出しを共用する

- **前提:** M05。
- **対象:** ledgerのget/cached/cache、recoveryの履歴query、共通repository。
- **手順:** reader側queryとcache SQLを移す。初期化はwriter、読出しはreaderの既存境界を保持。acceptedだけの成果物読出しと既存サイズ制限を残す。
- **検証:** accepted/unknown/cancelled各状態、存在しないindex、サイズ上限境界、再open後の履歴。V-RA(media)とTS復旧テスト。
- **完了:** 保存JSON・履歴shapeが旧fixtureと一致し、通常DBを別接続で初期化・更新しない。

### M07 — run状態とdownload枠をinstance化する

- **前提:** M06。
- **対象:** media `mod.rs`のEntry/RUNS/trim/cancel、`artifacts.rs`のDOWNLOADS、新しい状態所有者、desktop組立箇所。
- **手順:** 一つのMediaService用状態へ移す。desktopの全media呼出側へ同じinstanceを渡す。取消の先着記録、cleanup、同時実行上限、download枠を保持。
- **検証:** 既存の送信前取消・UUIDテストを移す。二instanceでrunが混ざらない、枠解放、上限、取消競合を追加。V-RA(media)とdesktopのmedia回帰。
- **完了:** 本番のRUNSとDOWNLOADSの正本が各一つ。単体テストがglobal resetや実行順に依存しない。

### M08 — LARM通信adapterを共通側へ接続する

- **前提:** M07。
- **対象:** generation/recovery内のLARM client組立、mediaのProvider adapter、既存`crates/larm-session/src/media/`。
- **手順:** larm-sessionを再利用し、route、資格情報、取消、進捗を明示入力にする。mockは同じ通信経路へローカルendpointを渡す方式とし、製品だけ別アルゴリズムにしない。
- **検証:** LARM既存回帰を維持し、認証失敗、timeout、切断後unknown、送信前取消、kind不一致を確認。V-RA(media)、larm-sessionを変更した場合はそのadvanceと利用側も。
- **完了:** Tauri/AppStateなしでadapter契約が実行できる。未知結果を成功や再送許可へ変換しない。

### M09 — 既存Replicate経路とテストを移す

- **前提:** M07。
- **対象:** `media_generation/replicate.rs`・`replicate_tests.rs`、共通adapterと旧利用側。
- **手順:** 既存実装を移し、global state・AppStateアクセスだけを確定APIへ置換。モデル解釈、job、取消、redirect/MIME、成果物制限を変えない。
- **検証:** 旧ケースと新ケースを一対一対応させ、GUI・実サービスなしで全移動ケースを実行。V-RA(media)と旧利用側回帰。
- **完了:** 画像labがLARM限定でもdesktopのReplicateは維持される。移植が大きい場合は移動と挙動変更を分け、後者をこの作業に混ぜない。

### M10 — 生成の調停をMediaServiceへ移す

- **前提:** M08、M09。
- **対象:** `generation.rs`の調停、新MediaService、desktop generate command。
- **手順:** route固定→予約→送信→進捗→成果物→採用の順を維持して移す。commandは入力・Channel変換だけを担当。DB・secret・Provider・EventSinkはB01の依存で渡す。
- **検証:** 同じrunId二重送信、前取消、送信後取消、route失効、成果物保存失敗、deadline、終端一回、Provider呼出回数をfixtureで確認。V-RA(media)、V-MIPC、TS生成回帰。
- **完了:** desktopの生成が共通serviceを使う。Channel配送失敗を自動的なProvider再送にしない。旧調停実装の正本を残さない。

### M11 — 残り4操作を同じserviceへ接続する

- **前提:** M10。
- **対象:** cancel command、`recovery.rs`、`artifacts.rs`、共通service。
- **手順:** 取消・履歴・再照合・成果物読出しを共通serviceへ順に接続。各操作の変更と対応テストを一組として進める。同期画像はunknown保持、remote jobは既存再照合を使う。
- **検証:** 保存済取消、未登録ID、採用済再照合、同期画像の応答消失、remote job復旧、binary成果物。V-RA(media)、V-MIPC、TS復旧回帰。
- **完了:** 5操作が同じservice・台帳・run状態を共有。操作間で別instanceを生成しない。4操作が一度に扱い切れなければM11a〜dとして同じ順に記録。

### M12 — desktop接続と既存テスト維持を受け入れる

- **前提:** M11。
- **対象:** media IPC登録、AppState組立、旧再公開、`scripts/verify-plan.ts`、対応テスト一覧。
- **手順:** 旧実装の一時コピーを除去。B00全ケースの配置・feature・gateを照合。feature付きmedia IPCを関連・全体advanceの契約stepとして明示登録し、planテストを追加。
- **検証:** V-RA(media)、V-MIPC、B00の利用側、V-BOUNDARY、V-ALL。大規模な接続変更として必要なV-FULLとdesktop smokeも記録。既存失敗で未到達なら受入未完了。
- **完了:** 画像・音楽・Replicate・履歴・取消のdesktop経路が成立。検証ケースの欠落なし。lab未実装でもアプリとして止められる。

## 6. 隔離HTTP host

### H01 — 隔離DBとhost組立を作る

- **前提:** M12。
- **対象:** 新規`services/feature-lab`、共通schema initializer、host統合テスト。
- **手順:** 空のlab DBと同じMediaServiceを組み立てる。必要tableの閉包をB00/B01から再確認し、共通schema定義を使う。資格情報は明示投入のみ。labのwriter所有・shutdownを定義。
- **検証:** V-RA(host)、隔離DBで予約・採用・監査・active確認を実行。通常DBパス拒否、secret未設定、再起動を確認。
- **完了:** saaa/Tauri/nativeを依存に持たず、本番DBを開かない。DBが起動しただけでは未完了。

### H02 — HTTPの履歴・再照合・成果物とアクセス制限を作る

- **前提:** H01。
- **対象:** host router、認証・Origin検証、履歴・再照合・成果物adapterとテスト。再照合は保存状態を更新し得る操作として扱う。
- **手順:** B01のHTTP仕様で3操作を実装。loopback、Host/Origin、session認証、body/成果物上限を適用。任意path/invokeは受け付けない。
- **検証:** ローカルfixtureで正当・不正認証、異なるOrigin、存在しないrun/index、未採用成果物、binary/MIMEを確認。再照合はfake Provider使用。V-RA(host)。
- **完了:** 同じservice結果を変換し、HTTP側の独自保存・状態遷移がない。

### H03 — 生成streamと明示取消を作る

- **前提:** H02。
- **対象:** hostの生成・取消adapter、run/task所有、stream契約テスト。
- **手順:** MediaService所有のrunを起動し、runId/順序/終端をHTTP envelopeへ変換。有限queue、切断処理、shutdownを実装。切断だけでは取消・再送しない。
- **検証:** 遅い受信、途中切断、重複要求、取消競合、終端重複、shutdown中run、再起動後unknown。Provider送信数をassert。V-RA(host)とmedia契約。
- **完了:** 5操作が揃い、request future消滅で台帳が成功扱いにならない。stream consumerが取消処理を塞がない。

### H04 — ブラウザ用HTTP adapterを既存パネルへ接続する

- **前提:** H03、U02。
- **対象:** 新規`src/features/media/mediaHttpApi.ts`、preview、HTTP adapterテスト。
- **手順:** 中立MediaApiを実装し、共通schemaで受信検証。runId/kind/順序/終端を検証して既存callbackへ渡す。切断時に履歴を再取得し、自動POSTしない。
- **検証:** V-TS、V-TSTで生成・復旧・新HTTPテストを別実行。mock streamで分割chunk、無効JSON、別runId、終端後進捗、binaryを確認。
- **完了:** 既存Reactパネルを両adapterで使える。Tauri APIをブラウザで呼ばず、frontendに正式なrun台帳を作らない。

### H05 — ロックを守る起動入口を用意する

- **前提:** H04。
- **対象:** `scripts/verification-build.ts`等の既存起動設計、新lab launcher、package scriptとそのテスト。
- **手順:** hostのbuildはverify経由・共有ロック内、完了後はロックを解放してbinaryを起動。再buildは再取得。Vite proxyとsession認証bootstrapを接続し、秘密をURL・ログ・Vite公開envへ置かない。
- **検証:** fake child processで順序、失敗時未起動、lock待ち、終了時の子process処理を確認。V-TS、launcherテスト、V-RA(host)。
- **完了:** 別target・直接Cargo・tauri devロック迂回がない。起動手順と停止手順を記録する。

## 7. 既存verifyのaffected

各ファイル名は提案。既存verifyを置き換えず、選択関数を小さいmoduleへ分ける。V01〜V06では実Cargoを何度も起動するテストを作らず、plan・Git fixture・fake processで分岐を検証する。必要な実行証拠はG02に集約する。

### V01 — 変更ファイル集合を収集する

- **前提:** B00。
- **対象:** 新規`scripts/verify-affected-changes.ts`、`tests/verify-affected-changes.test.ts`。
- **手順:** staged・unstaged・非ignore未追跡の和集合、rename旧新、削除元、明示baseからのcommit差分を取得。base不明・Git失敗は広い検証要求として返す。
- **検証:** V-TS、新テスト。空白を含むpath、rename/delete、未追跡、indexと作業ツリーの異なる変更、base不明、ignore入力をfixtureで確認。
- **完了:** 変更元をreportでき、未追跡だけの変更を空集合にしない。まだ検証を省略しない。

### V02 — 機能入力と所有者の表を作る

- **前提:** V01。
- **対象:** 新規`scripts/verify-affected-inputs.ts`と対応テスト、B00の所有者表。
- **手順:** package、TS feature、SQL、IPC、生成入力、asset、fixture、関連quality契約を対応付ける。未登録pathは全体fallback。秘密の内容は記録対象にしない。
- **検証:** media Rust、UIだけ、アバター描画だけ、registry、larm-session、schema、共通fixture、build.rs、lockfile、verify自身の各変更に期待所有者・契約集合を定義。
- **完了:** 「UIだけ」と言える理由が入力表にあり、Rustやnativeへの影響がある場合は省かない。

### V03 — 逆依存と変更前graphを解決する

- **前提:** V02。
- **対象:** 新規`scripts/verify-affected-graph.ts`と対応テスト。
- **手順:** 現在・変更前manifestの通常/dev/build依存を解析し、feature/target条件を安全側に扱う。消えた依存・packageも含む逆依存閉包を作る。解析不能はfallback。
- **検証:** 鎖・分岐・dev依存・target依存・依存削除・package削除・共通型のfixture。Cargoによるcompileと依存packageのtestを別項目で期待集合へ入れる。
- **完了:** 所有者と利用側の集合が説明可能。metadataを使う場合もverify内部・offline/lockedで取得し、取得失敗を成功扱いしない。

### V04 — 選択集合を既存planのstepへ変換する

- **前提:** V03。
- **対象:** `scripts/verify-plan.ts`、新affected planner、`frontend-tests.ts`の対象一覧受付、対応テスト。
- **手順:** normal/advance/fullの意味を保持して複数packageとTS・契約を合成。TS lint/typecheckは全体、TS testはファイル別process。generated/sizeと必要なquality/IPCを追加。
- **検証:** normalにbuild/testなし、advanceに必要stepあり、unknownは全体、重複除去で異なるfeature/stageを落とさない、TS process分離維持。V-TS、既存verify/frontend runnerテストと新planテスト。
- **完了:** `--package`を連結せず実行可能なplanになる。新CLIはまだ通常運用へ切り替えない。

### V05 — 並行編集で古くなった結果を検出する

- **前提:** V04。
- **対象:** `scripts/verify.ts`、既存lock/process境界、新fingerprint処理、対応テスト。
- **手順:** ロック取得後に入力を再確定。段階後・終了時の変更とwatcher通知を検出し、staleならOKを出さず有限回再計画。HEAD/index/未追跡/規則/toolchainを含める。
- **検証:** lock待ち中の編集、段階中の編集、未追跡追加、規則変更、連続stale、watcher失敗、検証自体の生成物の扱いをfake processで確認。失敗診断・即停止の既存テストを保持。
- **完了:** 編集を巻戻さずstaleを記録できる。開始終了hashだけで入力不変を証明したと報告しない。

### V06 — 説明表示・実行report・shadow入口を追加する

- **前提:** V05、B02。
- **対象:** verifyのCLI/help、report保存、新選択器のテスト。
- **手順:** 提案`affected`、`--level`、`--explain`を登録。通常成功はOK一つ、詳細はreport。shadowでは選択結果を記録し、従来の必要gateを実行する。
- **検証:** stdout完全一致、完全失敗診断、未実行step、選択理由、fallback理由、0件テスト、秘密値除外を確認。V-TS、既存verifyテストと新CLIテスト。
- **完了:** helpと実装が一致し、未実装optionを案内しない。過去結果キャッシュで検証を省略しない。

## 8. 段階の受入と効果確認

### G01 — 最初の画像切り出しを受け入れる

- **前提:** H05。
- **対象:** B00テスト対応表、共通media・desktop・HTTP・TSの契約fixture、検証report。
- **手順:** 同じfixtureで二hostの結果・進捗・取消・履歴・成果物を照合。mock Providerでブラウザから画像一件、取消、切断、再起動を確認。全既存ケースの行先・実行件数を照合。
- **検証:** V-RA(media/hostを別実行)、利用側、V-MIPC、TS各テスト、V-BOUNDARY、V-ALL、必要なV-FULL。単体ビルド記録にdesktop build.rs/native/sidecarがないことを確認。
- **完了:** GUI・実サービスなしの単体契約が通り、desktopも成立。未実施があればその受入は未完了。実Provider確認は別に明示された環境で行い、mock証拠と分ける。

### G02 — affectedの選択漏れを受け入れる

- **前提:** V06。mediaの新pathを評価する場合はG01も必要。
- **対象:** 期待選択集合fixture、shadow report、通常gate結果。
- **手順:** V01〜V05の全分類で期待集合が選択集合に含まれることを確認。既存全体advance/fullを維持して実行結果と照合。選択漏れは規則を直す。
- **検証:** 全体gateとselectedの成否一致だけで合格にせず、登録ケース・feature・command・入力manifestの対応を確認。共有verify変更としてV-ALLと必要なV-FULLを記録。
- **完了:** 日常入口にしてよい根拠と限界が残る。全体advance/fullの省略は許可しない。最初は結果キャッシュなし。

### G03 — 次のcrateを作る前に待ち時間を比較する

- **前提:** G01、G02。実装・受入の未完了があれば測定値に明示。
- **対象:** verifyの段階別report・必要な計測支援、方針書第11節の比較表。
- **手順:** B02の前値と同一条件で、変更なし、UI内部、media内部、共有型変更を比較。ロック待ち、compile/link、host起動、test、Provider外部待ち、最大RSS・同時メモリを区別。B02未測定の項目は後値だけを記録し、改善率を出さない。
- **検証:** 計測器追加もverify経由でテスト。キャッシュ全削除・別target・別checkoutで見かけ上の比較を作らない。安全な並列数で実施し、初回cold buildは必須にしない。
- **完了:** 短縮した時間、残る費用、未計測、全体gateへの影響が記録される。効果が小さい場合は次の分割を自動開始しない。

## 9. 後続候補の扱い

以下は選択前のbacklog。G03の結果から一つ選んで作業票を確定してから実装する。後続全体を一度にGrokへ渡さない。

| 候補 | 最初に渡す一件 | その後の順序・出口 |
| --- | --- | --- |
| task-queue | `task_queue.rs`の既存7ケースと横断利用側の対応表を最新化 | 実装＋7テストを移動 → 再公開・統合テスト接続 → 単体advanceと利用側受入。DB所有を変更しない |
| workspace | 既存manifest/lock/profile/feature差分の一覧だけを作る | 差分を確定 → virtual workspaceとlock統合 → verify選択互換の確認 → 全体受入。依存更新を混ぜない |
| Role Routing | reducerとその入力型・既存3ケースの依存を確定 | reducer単位で抽出 → 予算/recipeを別票で追加 → 利用側契約。Runtime全体を移さない |
| Context / Tools | broker全体ではなく純粋なranking・予算等の一関数群を選ぶ | 型の所有先確定 → 実装と既存ケース移動 → adapter接続 → 利用側受入 |
| Memory / Conversation / Work | 一つの業務更新のtransaction・取消・採用を図にする | 現行保証のfixtureを先に確定し、関数群単位の作業票を追加。巨大ドメインの丸ごと移動は禁止 |
| Voice | 既存PCM fixture・実機受入とI/O所有者を対応付ける | 録音/再生を同じVPIO所有者に維持した抽出だけ検討。実機条件が用意できなければ実機受入未完了 |

## 10. そのまま渡す依頼文と完了報告

次のテンプレートの作業IDだけを選び、担当モデルへ渡す。これは依頼文の案であり、この文書の作成によってGrokや別タスクへ送信したものではない。

```text
SAAAの作業票 <ID> 一件を実施してください。
リポジトリ: /Users/y.noguchi/Code/SAAA
計画: docs/plans/rust-domain-crate-execution.md
設計根拠: docs/plans/rust-domain-crate-migration.md

最初にAGENTS.mdと両文書の共通契約、対象作業票、進捗記録を読む。
initial_instructionsはこの会話で未実行の場合のみ実行する。
前提IDの完了と現在のコードを確認してから対象範囲だけ変更する。
保存設定・Provider・本番DB・音声契約を変更しない。
既存テストを移動・維持し、検証はverify経由とする。
他のタスクへ依頼せず、次のIDを自動で始めない。
想定外の境界変更が必要なら、理由・対象・追加作業票を記録する。
完了報告は下の形式で、実施と未実施を区別する。
```

```text
作業ID:
状態: 未着手 / 実装中 / 実装済・受入未完了 / 完了
前提IDと確認結果:
基点HEAD・関連する既存変更:
変更ファイルと各変更の理由:
維持した契約・旧実装の削除/再公開先:
旧テスト → 新テスト → feature → gate の対応:
実行コマンド・対象・実行件数・結果:
失敗と未実施（全体gateを含む）:
一時実装・残課題・完了させる作業ID:
次に着手可能なID（自動着手しない）:
```

完了判定は、変更ファイルの存在だけではなく、作業票に記載した振る舞いと実行証拠で行う。未実施の検証が残る場合、実装が終わっていても該当受入を完了にしない。
