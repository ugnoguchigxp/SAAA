# SAAA Agent Connection provide 契約

SAAA の会話と画像・楽曲サービスの発見には、それぞれ公開 selector `SAAA`、`SAAA-w-Image`、`SAAA-w-music` を使う。保存済みの旧具体 Profile ID や未対応の値は、実行時に `SAAA` として解釈する。設定値自体は書き換えない。

`POST /v1/agent-connections` が provide 要求である。通常会話の body は次の通り。画像・楽曲を広告するConnectionでも `profile` だけを対応するselectorに変える。Connection作成は生成サービスの起動契機ではない。生成だけを行う画面はConnectionを作らず、Profileの`services`から公開APIを発見する。

```json
{"profile":"SAAA","audience":"saaa-desktop","client":"saaa-desktop","ttlSeconds":900,"allowFallback":false,"deploymentPolicy":"existing-only"}
```

作成要求には待機時間を指定する`Prefer`（共通セッションは`wait=1`、dynamic_lanは`wait=300`） とセッションごとの `Idempotency-Key` を付ける。任意の `GET /v3/agent-profiles?profile=<selector>` を実行したときだけ、その revision を `expectedCatalogRevision` に含める。具体 `agentProfile` は応答で検証する値であり、次の作成要求には使わない。

201 は ready と全 Provider の claimable を検証する。202 は Location の同じ Connection を期限付きで poll する。Warm ProviderはProfileに広告されたllm、asr、tts、embeddingとSystem One（`larm.system-one.v1`）を区別して保持する。旧構成のbackchannelにも対応する。全件接続では広告されたWarm Provider集合を検証する。llmだけを要求するtext adapterではllmだけの応答と旧サーバーの全Warm応答を扱い、名前・protocol・endpoint・model・必要なcontextWindowを確認する。モデル名はProfileとclaimから取得する。画像と音楽の追加サービスは `services` で検証し、claim 対象にしない。claim 後の短期 credential、baseUrl、model を推論の正本とし、作成応答と矛盾した場合は推論を止める。終了時は DELETE で release する。

自己診断は LARM を機能（応答・音声認識・音声合成・埋め込み）の経路として扱い、カタログでの提供確認（実行は未確認）と、Session を claim して行う実推論の確認を別の証拠として区別する。HTTP の 4xx 応答は到達不能として扱わない。API token と claim token は診断結果、SQLite、設定画面、ログに保存しない。

## 要求時に起動する画像・楽曲サービス

`GET /v3/agent-profiles?profile=SAAA-w-Image`または`SAAA-w-music`の`services`を使う。画像は`media.image.generate` / `larm.image-generation.v1`、楽曲は`media.music.generate` / `larm.music-generation.v1`を選ぶ。`endpoint`と`model`は発見結果から読み、SAAAには生成先のパスやモデル名を固定しない。`startupPolicy`も保持する。`minWarmInstances: 0`は要求時起動を意味し、停止中という理由で利用不可にしない。旧サーバーでこの属性が省略されていても、生成要求前の起動・health probeは行わない。

画像生成のJSON形式は現行OpenAPIに定義がないため、下表の画像要求・応答は暫定対応である。更新版のAPI定義または応答例との照合が残る。

| 操作 | 公開APIの使い方 |
| --- | --- |
| 画像生成 | 発見したendpointに`{prompt, model}`をPOST。同期応答の`artifacts`または`data[].artifact`から永続artifactの`id`、`contentUrl`、`mimeType`等を取得する。Warm Providerがreadyでも画像生成成功とは判定しない。 |
| 楽曲生成 | 発見したendpointにprompt、発見したmodel、durationSeconds=180、instrumental=false、outputFormat=mp3、quality=balancedをPOST。202の`jobId`を保持する。 |
| 楽曲の追跡 | POST応答にLocationがあればそのjob URL、なければ発見した生成endpointの`/{jobId}`をGET。queued/loading/generating/encodingを表示し、completedのresultから`audioUrl`と`metadataUrl`を取得する。failed/cancelledは成功にしない。 |
| 楽曲の中止 | 同じjobリソースをDELETE。cancelledを確認できなければ停止完了と表示しない。POST中に中止された場合も、POST応答からjobIdを受け取って中止を依頼する。 |
| 成果物取得 | artifactのURLをGET。楽曲はmetadataのidを確認してからaudioを取得。画像と音声を表示・再生・保存できる。取得失敗は生成成功と分け、GETだけを再試行する。 |

すべて保存済みHarnessの公開API originとcontrol credentialを使う。別originやProviderの別ポートへのURLは拒否し、credentialをフロントエンドに渡さない。Connection取得やProfile発見では生成POSTを送らず、systemd操作・事前起動・TTL延長要求・Coldサービスの定期health checkを実装しない。画像120秒、楽曲300秒のidle TTLと生成後の停止はLARMが管理する。

生成POSTの待機上限は画像・楽曲とも15分、楽曲jobの追跡上限は受理後30分。追跡GETは2秒間隔（各要求30秒）、成果物の取得は120秒。楽曲の追跡期限ではDELETEを試みる。画像の中止は同期応答の待機を終了する操作で、遠隔生成の停止は保証しない。POSTの通信切断・タイムアウト・画像の待機中止・楽曲中止未確認では、生成が継続している可能性と重複再送の可能性を表示する。

生成POSTは自動再送しない。409等の重量級サービス競合は生成失敗と再試行可能なことを表示し、利用者の再試行操作で新しい要求を送る。jobの状態確認だけが失敗した場合は生成失敗と断定しない。同じ画面から同時に送信できる要求は1件、backend全体では2件までで、画像・楽曲の起動排他はLARMの競合応答に従う。

成果物の参照はアプリ内で最大16要求分を保持し、古い完了要求から破棄する。再起動後のjob復元・履歴管理はこの実装に含めない。必要な成果物は保存操作で取得する。JSON応答は1MiB、1要求の画像は8件まで、各画像32MiB、音声128MiB、backendの同時取得は2件に制限する。

## 変更箇所と検証

- `crates/larm-session/src/catalog.rs` / `contract.rs` / `lib.rs`: Provider集合とservicesの保持・検証、System One、startupPolicy、発見先の比較。
- `crates/larm-session/src/media.rs` / `media/transport.rs`: ColdサービスのPOST、job追跡、中止、成果物GET。Warm接続から独立して実行する。
- `src-tauri/src/providers/dynamic_lan/`: 既存接続でも発見したサービスを保持し、固定の生成パスを要求しない。
- `src-tauri/src/media_generation/`: 保存済みHarnessと既存credentialを使うIPC、重複要求の拒否、boundedな結果参照。
- `src/features/media/`: 会話画面の画像・楽曲生成、待機、中止、手動再試行、成果物の表示・再生・保存。

ローカルHTTP fixtureでCold待機、起動失敗、競合、生成失敗、成功、artifact取得、タイムアウト、中止、POST非再送を確認する。Providerの接続成功だけでは生成成功にしないケースも含む。WorldModelとMemoryの実装・設定は変更しない。

2026-10-01に保存済みHarness `http://192.168.0.130:9810` で実生成まで検証した。発見結果はまだOrnithの会話Profileで、System OneとservicesのstartupPolicyがなく、画像生成POSTはOpenAPIにも掲載されていない。画像は広告されたendpointへ1回POSTして404 `not_found`、楽曲は1回POSTしてjob `music_e9832a72-4ab0-4aa2-825a-e9192697c298`を受け取り、queued→failedを追跡した。楽曲の失敗理由はLARM内部から生成先に接続できないことだった。自動再送、systemd操作、事前起動は実行していない。既存Warm接続は5 Providerのhealth、会話LLM・embedding・TTS・ASRの実API応答、接続解放、短期credentialの解放後401を確認して成功した。ただし現行のOrnith構成での結果であり、新しいGemma 4 / System Oneの成功とはしない。成果物は生成されず、実環境での成功・成果物取得・新構成のCold起動・排他の確認は未完了である。

### 機能別の確認

| 対象 | 確認内容 | 結果 |
| --- | --- | --- |
| Profile / Connection | Warm集合、System One、停止中のCold services、startupPolicy保持、発見先・モデル照合、作成だけでは生成しない | 共通crateとdynamic_lan fixtureで成功 |
| 画像 | 同期待機、起動失敗、競合、生成失敗、成功、成果物GET、中止、タイムアウト、POST非再送 | fixtureで成功。実APIは404 |
| 楽曲 | queued/loading/generating/encoding/completed、起動・生成失敗、DELETE、中止未確認、追跡通信失敗、期限 | fixtureで成功。実APIはjob受理後に生成先への接続失敗 |
| 成果物 | 同じ公開origin、別jobのmetadata拒否、Content-Type検証、取得だけの再試行、画像表示・音声再生・保存 | backendと画面テストで成功。実成果物は未取得 |
| IPC | 3コマンドの実dispatch、入力検証、早期中止、既存コマンドへの委譲 | 隔離したTauri mock runtimeで3件成功 |
| 画面 | 待機、中止、競合の手動再試行、失敗の区別、不明な結果の重複警告、成果物取得、離脱後の応答 | 6件・42 assertions成功。既存会話画面も1件・22 assertions成功 |

ローカルの共通crateは単体75件と統合7件が成功し、live test 5件は通常実行から明示的に除外する。dynamic_lanは32件成功、既存live 1件はignored、既存World連携の`wr_t13_real_allocation_refreshes_source_frame_and_releases`は除外している。この既存テストは今回の変更を外した隔離コピーでも失敗し、現行のtool-less transportがcontext/persistenceを拒否することが原因である。WorldModel/Memoryの実装を変更して回避しない。

### 静的検査と全体ゲート

型検査、lint、フロントエンドビルド、IPC binding 8件、共通crate全体の書式とClippy `-D warnings`、変更したネイティブ／画面ファイルの書式、変更モジュール18件のサイズ検査が成功した。新規ファイルと未登録だった会話画面2ファイルのサイズを登録し、既存のサイズ基準はリセットしていない。変更しないpersonal-state-coreの既存ゲートも106件成功した。

全体ゲートは成功していない。検証中にWorld/MemoryとTTS辞書が並行変更された。型・lintの最後の再実行は、`maintenanceSchema.ts`の編集中の構文不一致と`useTtsDictionarySync.ts`のcleanupのref参照エラーで停止している。上記の型・lint・build成功は、その並行変更前の検証時点の結果である。IPC bindingの再検査は一度、並行変更中のTTS辞書のコンパイルエラーで停止した。その後の再実行で8件成功を確認した。ただし全体ゲートの成功を意味しない。`check:local`は既存36ファイルの書式で停止するため、後続の検査を個別にも実行した。全体のRust書式、Clippy ratchet、サイズ検査には今回の変更外の不一致が残る。フロントエンド全件は389件成功・40件失敗・読込エラー15件。今回の画面変更を外した隔離コピーは382件成功・41件失敗・読込エラー15件だった。追加テストによる他ファイルへのAPIモックの干渉は修正済みである。

Rust全件は約10分で打ち切った時点で1632件成功・47件失敗・27件ignored・6件未完了であり、完走した結果として扱わない。旧会話機構の削除に追従していないテストが含まれ、quality runtimeも同じ理由で1件失敗する。範囲外の旧機構や音声・WorldModel/Memoryの動作を変更して全体ゲートを通すことはしていない。

機能と全体の結果、実APIの応答、残っている検査項目は `spec/evidence/larm-cold-services/2026-10-01-verification.json` に保存した。実環境の成功条件は、更新版LARMがGemma 4 / System One / startupPolicyを広告し、画像POSTと楽曲jobが完了して、その成果物のmetadataとcontentを取得できることである。Profile取得やWarm接続の成功だけでは満たさない。

コミット直前には、このタスクの変更だけを一時Git indexへ選び、共有ファイルのWorld/Memory登録・コマンド追加を除外した。そのstage内容を隔離コピーに展開し、型検査、lint、変更画面の書式、共通crateの書式・Clippy・82件、生成IPC 3件、生成画面6件、会話画面1件が成功した。並行作業中のファイルを含まないコミット対象についての確認であり、更新版LARMの実生成成功の代わりにはしない。

### 再検証

| コマンド | 成功条件 |
| --- | --- |
| `cargo test --locked --manifest-path crates/larm-session/Cargo.toml` | 単体75件・統合7件成功。通常実行ではlive 5件ignored |
| `cargo fmt --check --manifest-path crates/larm-session/Cargo.toml` / `cargo clippy --locked --manifest-path crates/larm-session/Cargo.toml --all-targets -- -D warnings` | 除外なしで書式・静的解析成功 |
| `cargo test --manifest-path src-tauri/Cargo.toml --features conversation-queue-e2e --lib media_generation::` | 実IPC dispatchを含む3件成功。DBはメモリ上の隔離fixture |
| `bun test tests/media-generation.test.tsx` / `bun test tests/conversation-queue-page.test.tsx` | 生成画面6件、会話画面1件成功 |
| `bun run typecheck` / `bun run lint` / `bun run build:frontend` / `bun run ipc:check` | エラーなし |
| `bun run check:local` / `bun run test:frontend` / `cargo test --manifest-path src-tauri/Cargo.toml --lib` | 全体の残課題を別途確認。現時点では成功扱いにしない |
| `cargo test --locked --manifest-path crates/larm-session/Cargo.toml --test media_live -- --ignored --nocapture --test-threads=1` | 発見→実生成→metadata/content取得が画像・楽曲の両方で成功 |

live実行には`SAAA_LARM_CONTROL_URL`と安全に読み込んだ`LARM_API_TOKEN`が必要。画像・楽曲を順番に1回ずつ実要求し、楽曲は現行画面と同じ180秒の設定を使う。失敗を自動で再送せず、LARM更新と生成の停止状態を確認したうえで明示的に再実行する。現在は広告endpointの404と生成先接続失敗が確認されているため、同じサーバーへの反復要求で成功扱いにしない。
