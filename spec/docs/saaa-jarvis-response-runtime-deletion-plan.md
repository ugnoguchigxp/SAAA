# SAAA：旧回答Runtimeの削除計画

作成日：2026-09-26。状態：旧実装本体を削除。A〜Eの旧実行器はinventoryの対象pathから撤去し、F/Gの通常会話入口・UI turn制御・Butler実行・World注入を切った。新Sessionは未実装で、現在の作業ツリーはビルド不可。DB・設定は変更していない。

対象HEAD：`52207165d4d7aa8c962320164c9551a8ce8b4c11`。開始時の差分は前の依頼で作成した[再構築提案](saaa-jarvis-response-runtime-rebuild-proposal.md)だけ。以降にHEADや対象ファイルが変わった場合は棚卸しを照合し直す。

体験の正本は[5 Providerコンセプト](saaa-jarvis-five-provider-concept.md)。今回は回答制御の作り直しに加え、ユーザーの追加指示により **Memory・World Modelと会話の接続を一旦切る**。保存済みの記憶やWorldデータの削除は含めない。

## 1. 削除の目的と終了状態

削除するのは、ASRの受付、一次応対、思考への引継ぎ、最終回答、TTSの順序・寿命を複数の場所で制御している旧構造。新しい回答経路には、旧Role Routing・Butler・LFM/Qwen受付・UI側turn制御を呼ばせない。

削除完了の条件は次の通り。

1. 通常会話の音声・テキスト入力は新Session入口だけに届く。coding等の別用途を通常会話のfallbackに使わない。
2. ASR待受け、Qwen一次応対、ornith仕事、TTS/再生はそれぞれ一つの所有者を持つ。
3. 旧会話の入口・タイマー・自動再試行・起動復旧・音声発火経路が実行可能なまま残っていない。
4. Provider資源確保・認証・音声デバイスは会話から独立し、5役割を利用できる基盤が残る。
5. 新しい通常会話からMemory検索・抽出・更新、World取得・更新・Context注入を呼ばない。embeddingも新回答の依存にしない。
6. 履歴、設定、資格情報、話者登録、監査を維持し、実ASR→Qwen→ornith→実TTSが旧回答コードなしで動く。

ファイル数の削減だけでは完了にしない。旧Runtimeを別名のwrapperに隠す、旧経路へ自動fallbackする、旧状態を新状態へ二重書込みする方式は採らない。

## 2. 棚卸しの範囲

[inventory.json](../evidence/response-runtime-deletion-20260926/inventory.json)に関連144ファイルのパス、分類、SHA-256を記録した。**144ファイルすべてを削除する一覧ではない。** 下表のA〜Iに対応する、変更判断の対象一覧である。静的検索と主要実装の読取りに基づき、完全なコンパイラcall graphや実行到達性の証明ではない。

ここでの「撤去」は利用者からの入口・実行処理をなくすこと。「移設後撤去」は必要な低水準機能を小さな境界へ移して元の実装をなくすこと。「部分撤去」は共有ファイルの会話制御だけを外すことを指す。表の省略パスはリポジトリルート基準、実在する全パスはinventoryに展開済み。

| 分類 | 主な対象 | 処置・削除前の条件 |
|---|---|---|
| A：旧一次応対と発話制御 | `providers/larm_voice/{frontdesk,frontdesk_decision,frontdesk_echo,frontdesk_repository,decision,response,speech_priority}.rs`、`runtime/{voice_response,voice_response_state,reasoning_ack}.rs`、`src/lib/lfmConversationRuntime.ts` | 撤去。登録IPC、診断、生成後callback、旧DB migrationを先に分離。原文保持・取消・重複防止の回帰ケースは新実装へ移す |
| B：会話と混在した資源Owner | `providers/larm_voice/{mod,audio,profile}.rs`、`src/lib/larmVoice{Owner,Runtime,Drain}.ts`、`useLarmVoiceLifetime.ts`、`useLarmConnectionStatus.ts` | 資源機能を移設後、旧会話Ownerを撤去。lease key、cleanup責任、shutdownを保持。UIは新資源状態を表示するだけにする |
| C：旧通常会話の実行経路 | `runtime/conversation_turn.rs`、`conversation_provider_route*`、`conversation_inputs*`、`conversation_prepare.rs`、`conversation_context.rs`、`conversation_stream.rs`、`conversation_controller/`、`conversation_role_steps.rs`、`role_step_sink.rs`、`conversation_state_answer/card.rs`、`providers/stream/larm_voice.rs` | 通常会話から撤去。他用途に必要な処理が残れば先に移設。新Runtimeからこれらを呼ばない |
| D：旧ASR待受け・入力配送 | `src/features/voice/useAmbientVoiceSession.ts`、`ambientVoiceCapture*`、`voiceFinalDeliveryQueue.ts`、`src/lib/voiceAsrRuntime.ts`、`voice/streaming_asr/{commands,manager,session,route,batch_runtime}.rs`、`voice/session/asr*.rs`、`runtime/voice_frontend.rs`、`persistence/audit/voice_frontend_observer.rs` | Listeningへ置換して旧寿命管理と会話配送を撤去。デバイス・話者登録・プロトコル処理は分離。監査は観測だけにし、監査callbackから別の会話実行を作らない |
| E：旧TTS実行器と発話順序 | `voice/streaming_tts*`のruntime・chunker・fallback、`voice/session/tts*.rs`、`role_routing/speech_queue.rs`、`speech_repository.rs` | Speakingへ置換。句読点即時合成と単一playerへ統一。共有通知と停止の呼出元を移してから旧実行器を削除 |
| F：共有入口・UI・保存 | `runtime/{start_turn,turns,event_hub}.rs`、`runtime/turns/`、`butler_loop/`の接続・台帳、`lib.rs`、`app_state.rs`、`persistence/{schema,app_commands}.rs`、`providers/session_store.rs`、`ChatPage.tsx`、`useConversationTurn.ts`、`conversationTurnControls.ts`、`reasoningRun*`等 | 部分撤去。coding dispatch、履歴表示、DB接続、汎用取消まで一括削除しない。通常会話の入力保留・再起動・相槌・音声開始判断を外す |
| G：Memory・World接続 | `useWorldScope.ts`、`WorldScopeSelector.tsx`、`requiredContextRecovery.ts`、`runtime/context/`のbroker・scope・world経路、`providers/chat_completions/{mod,generation,world_body}.rs`、`providers/stream/attempt.rs`、`memory/personal_state/{output,product_binding,worker}.rs` | 新会話への接続を切る。共有ファイル・管理機能は保持。会話成立をMemory/Worldの接続・scope・生成許可へ依存させない |
| H：純粋処理・I/Oの再利用候補 | `qwen_control_stream.rs`、ASRのbatch engine・reconciler・speaker gate、PCM packetizer/sender、capture resources、native/worklet bridge | 自動的には削除しない。依存と契約を確認し、純粋処理だけ移設可。既存の判定アルゴリズムが正しいという保証にはしない |
| I：登録・試験・文書 | IPC契約・生成binding・tests、size/clippy基準、Runtime/Voice README、旧実装計画 | 実装置換と同じ変更単位で更新。古い構造を復元させる指示を残さず、試験対象を消して合格を装わない |

`runtime/`、`providers/`、`voice/`、`role_routing/`をディレクトリごと削除する操作は計画に含めない。

## 3. 実際に見つかった削除前の依存

### 3.1 LARM Ownerを先に消せない

現在の [larm_voice/mod.rs](../../src-tauri/src/providers/larm_voice/mod.rs) は、create/claimだけでなく会話ID・UI所有者・TTS終了を扱う。次の消費先がある。

- [ASR routes](../../src-tauri/src/voice/session/asr_routes.rs) と [TTS要求](../../src-tauri/src/voice/http_audio/requests.rs) の資源取得・再接続。
- [Memory product binding](../../src-tauri/src/memory/personal_state/product_binding.rs) の共有session取得。
- [Provider診断](../../src-tauri/src/diagnosis/checks/harness.rs)、[Dynamic LAN接続](../../src-tauri/src/providers/dynamic_lan/connection_lifecycle.rs)、[TTS catalog](../../src-tauri/src/voice/tts_catalog.rs) のprofile選択。

対処：Provider Resourcesへ接続・profile・credential・leaseの機能を移し、各消費先を付け替える。Memoryを会話から外しても、Memory管理機能の資源取得まで壊さない。`crates/larm-session` の正式なcreate→poll→claim→request→releaseは保持する。旧Ownerの責任を捨てて別Ownerを二重作成しない。

### 3.2 旧LFM受付は、通常UIで呼ばれていなくても登録が残っている

[lfmConversationRuntime.ts](../../src/lib/lfmConversationRuntime.ts) の `receiveLfmUtterance` / `speakLfmReply` は、調査した `src` 内の検索では定義以外の呼出しが見つからなかった。一方、[command_registry.rs](../../src-tauri/src/runtime/command_registry.rs) には `receive_lfm_utterance` / `speak_lfm_reply` が登録され、[schema.rs](../../src-tauri/src/persistence/schema.rs) から旧repositoryのmigrationが呼ばれる。[harness_llm_diagnostic.rs](../../src-tauri/src/harness_llm_diagnostic.rs) も旧判定器を使う。

対処：未使用と断定してファイルだけ消さず、IPC公開・診断・schema依存も一緒に撤去/分離する。DB初期化に必要な旧DDLは互換schemaへ移す。通常入口以外の呼出し有無はfeature付きbuildとIPC一覧でも確認する。

### 3.3 TTSの所有者は通常回答以外からも呼ばれる

`StreamingSpeechRuntime` は [steward/report/publish.rs](../../src-tauri/src/steward/report/publish.rs)、[Memoryのforget処理](../../src-tauri/src/memory/personal_state/commands.rs)、[app_commands.rs](../../src-tauri/src/persistence/app_commands.rs)、[voice_behavior/run_state.rs](../../src-tauri/src/voice_behavior/run_state.rs) 等から参照される。

対処：新Speakingに対して、通知の投入と対象音声の停止を行う小さい境界を用意する。通常回答の再生成器へ通知を通す必要はない。forget・取消に伴う失効音声の抑止は残す。移行完了まで旧・新playerを同時に所有させない。

### 3.4 Role Routing・Butler・Event Hubは共有部分を含む

[turns/execute_turn.rs](../../src-tauri/src/runtime/turns/execute_turn.rs) はcoding・capability・通常会話を振り分ける。[tool_selection/gateway/authorize_routing_tool.rs](../../src-tauri/src/tool_selection/gateway/authorize_routing_tool.rs) はRole Routingを操作権限に利用する。Butler台帳はmessage提示確認やrun終了からも呼ばれる。

対処：通常会話を新Sessionへ振り分けた後、旧通常会話のcontroller/role実行/継続ループを撤去する。coding、Tool権限、監査・過去rootの表示は維持する。Role Routingディレクトリ全体の撤去は、これら共有機能の後継が必要であり今回の一括削除対象にはしない。新会話から到達しないことを確認する。

## 4. Memory・World Modelの切断範囲

単に `enabled=false` を深い階層へ渡す方式ではなく、新Sessionから以下の呼出しを外す。

| 接続点 | 切るもの | 残すもの |
|---|---|---|
| 起動/初期化 | 会話開始のためのMemory製品接続・World readiness待ち。隔離した新hostからの自動抽出worker起動 | 設定・記憶の管理機能、必要な明示的forget/cleanup。他用途のworkerは新会話を自動処理しないよう分離 |
| 入力受付 | `prepare_runtime_run` 等からのMemory抽出制御、長期Context組立、World scope解決 | 入力原文の保存、会話IDと所有権の検証、長さ上限 |
| モデル要求 | `conversation_prepare` のMemory compose、World compose/revalidate、scope候補、検索・embedding、Tool定義注入 | 固定指示、今回の原文、上限付きの新セッション内会話、仕事の実状態。必要な入力長制限 |
| Provider送信 | 既存chat_completions経路のWorld body/generationへの必須依存 | 認証、正式なlease、HTTP/SSE処理。新adapterは旧会話用ModelStreamContextを丸ごと受け取らない |
| 回答採用・保存 | Memory generationの成否を回答公開の必須条件とする接続、回答を契機とする自動抽出/World更新 | 新Sessionの取消・版・所有権・メッセージ存在確認と原子的な保存。無条件の公開許可にはしない |
| UI | 通常会話からのWorld scope選択・pollと、その失敗による会話停止 | 過去の会話・記憶の管理画面。新会話には「記憶・状況参照は未接続」の実効状態を表示 |

既存の[出力公開処理](../../src-tauri/src/memory/personal_state/output.rs)と[保存処理](../../src-tauri/src/providers/session_store.rs)には `personal_state::generation::allow_run` が入っている。その呼出しを単に成功へ置き換えず、新会話側で取消・削除・古い結果の排除を行う。旧共有経路を引き続き使うcoding等の保護は維持する。

会話ログの保存・表示はMemory利用と分ける。履歴が画面に残ることを、長期記憶を参照できる状態と扱わない。MemoryやWorldの質問には未接続であることを示し、取得したように答えない。

切断の合格条件：Memory/Worldのサービスが不通でも新会話が起動し、Qwen・ornith・TTSの経路が動く。新セッションの相関IDでMemory/World/embedding要求・抽出予約・更新が0。無関係な他用途の通信と混同せず確認する。既存の全設定値をリセットして成立させない。

## 5. 残す基盤・データ

- **Provider資源**：`crates/larm-session/`、Dynamic LANの認証・catalog・接続契約、Provider設定と資格情報保存、5役割のcapability。保持は現状性能の保証ではなく、新Runtimeとは別に検証する境界という意味。
- **音声I/O**：`voice/audio_backend/`、PCM変換、HTTP音声の取得・decode・低水準player、system TTS等の選択済みadapter、話者登録・モデル・プロファイル。音声機器とAEC参照の所有者は一つに保つ。
- **履歴・管理**：会話本文、設定、監査、Memory・Worldデータ、記憶削除の導線、coding・steward・scheduleの保存と明示的な操作。
- **再利用候補**：Qwenストリームparser、ASR文字列整合、packet番号・世代排除などの純粋処理。新ドメインへ持ち込むのは小さい入出力契約だけ。旧セッションやAppStateをそのまま持ち込まない。

TTSの既存chunkerには最低長・目標長等の方針があるため、単に移動して句読点即時合成を満たしたとしない。ASRのreconcilerや話者判定も、原発話の欠落を起こさないという保証は未確認。処理を残す場合は音声fixtureと実機で評価する。

## 6. 実行順序

これは今後の削除実装の順序であり、今回実行した作業ではない。各段階はbuild可能な変更単位にする。

| 段階 | 作業 | 次へ進む条件 |
|---|---|---|
| D0：保存・対象確定 | HEAD/差分、inventoryのhash、実効build、既存チェック結果を保存。DBは読み取り専用接続からSQLite backupで整合したコピーを作る。既存の起動session/要求を把握する | 差分と復元先が確定。失敗中のテストを新規回帰と区別できる。稼働DBのdbファイルだけを単純copyしない |
| D1：共有基盤の切出し | Bの接続/lease/profile、低水準音声I/Oを独立境界へ移す。診断・Memory管理・通知等の共有呼出元を付け替える | 資源契約・設定互換・cleanupの試験が通る。旧回答を介さず資源を扱える。共有Ownerの二重作成0 |
| D2：新経路の隔離と接続切断 | 新Sessionを隔離hostへ配線し、Memory/World、旧Role Routing/Butler、旧UI turn制御を接続しない。通常UIの旧経路とはsession/DBを分ける | 新経路の依存・通信監査で旧回答処理/Memory/World呼出0。準備失敗も有限で表示される |
| D3：最小経路の実機合格 | 再構築提案R0の実ASR→Qwen→TTSと実ASR→Qwen→ornith→TTSを確認。生成中のQwen応対と停止も確認 | 最終回答の保存・可聴再生まで実測。mockやHTTP疎通だけを合格にしない。ここは削除本体へ進むための依存作業 |
| D4：入口切替と旧本体削除 | 旧入力受付を閉じ、稼働中処理の終了/取消と音声停止を確認。通常会話の入口を新Sessionへ切替。A/C/D/Eの旧制御とFの会話枝を削除 | 旧IPC・timer・worker・復旧起動がなく、同一入力/資源/音声の所有者が一つ。codingと履歴等は維持 |
| D5：残存物の撤去・受入 | 未参照import、旧feature/env分岐、登録、試験、型、文書を整理。隔離DBのmigration・実機回帰を実施 | 新旧fallbackなし、旧会話実行シンボルへの参照なし（履歴migration/検証fixture等の明示例外を除く）、所定チェックと実機合格 |

D4をD3より前に実行して通常利用の音声経路を全て失う運用は、この計画では採らない。新実装はD2から旧経路を一切使わず、旧コードの存在が新構造を規定しないようにする。高度な機能の完成をD4の条件にはしない。

### D4の内部順序

1. 旧受付を閉じ、ASR→旧submitPrompt、再描画/再接続からの再投入、旧Butler未消費入力の自動消費を止める。
2. 実行中のrun、ASR session、TTS player、claim準備をdrain/取消する。取消要求と遠隔停止確認を分け、終了不明のcleanup責任を保持する。
3. 新Sessionを排他的な所有者にする。旧runや旧pending入力を新仕事として自動再実行しない。必要な中断状態を履歴に残す。
4. `receive_lfm_utterance` / `speak_lfm_reply`、旧音声配送、旧Frontend/handoff/filler/response制御を登録と呼出元ごと削除する。
5. ASR待受けmanager、TTS scheduler、会話UIの旧状態を後継へ付け替え、旧ファイルを削除する。共有ファイルは会話枝だけを削る。
6. 旧会話用role/Butler実行・復旧を撤去し、残存参照を検査する。shared Tool権限や過去台帳表示は範囲を限定して残す。

切替をfeature flagの組合せで常設しない。開発中の選択はsession開始時の一回だけにし、切替完了後は旧通常会話の実行先自体をなくす。

## 7. DB・設定の扱い

**この削除計画で本番テーブルのDROPや保存データのDELETEは行わない。** コードの撤去とデータの物理削除は別作業にする。

| 保存物 | 方針 |
|---|---|
| `settings_documents`、`credential_secrets`、話者登録 | 保持。保存済みProvider/role設定は、旧項目が新Runtimeで未使用になっても勝手に消さない。実効設定を区別する |
| `larm_voice_lease_slot` | 接続所有権の情報として継承。claim/release確認前に削除・初期化しない |
| `conversations`、`conversation_messages`、`runtime_runs`、`provider_sessions`、transport/audit | 履歴・証拠として保持。新経路の保存境界を明確にし、旧runへ新回答を書かない |
| `lfm_voice_utterances` | 新会話からの書込・実行claimを終了。旧DDL/migrationだけ互換schemaへ移して、旧repositoryの実行コードを削除可能にする |
| `conversation_work_state`、`conversation_work_runs`、`conversation_run_inputs`、`conversation_run_input_gate`、`conversation_events`、提示確認 | 旧会話のwriter/consumerを終了。coding等の共有利用と過去表示が残る部分は保持。新仕事の正本へ流用して二重管理しない |
| `rr_*`、`speech_deliveries` | 過去履歴・共有用途は保持。新会話と旧会話のどちらが書くかを固定し、新会話に旧active root制約を要求しない |
| Memory・World・vector index | データは保持。新会話からのread/update/extractを切り、再接続は後段の独立作業にする |

migration試験は空DB、現行DBの隔離コピー、旧LFM列を含むfixtureで行い、`PRAGMA integrity_check` と `PRAGMA foreign_key_check`、履歴件数・設定fingerprint・話者登録を確認する。新しい仕事台帳が必要なら追加migrationとして設計するが、削除段階で旧テーブルを破壊して合わせない。

## 8. 試験・生成物

### 必須の確認

| 変更領域 | 実行するチェック | 確認すること |
|---|---|---|
| 資源層 | `cargo test --manifest-path crates/larm-session/Cargo.toml` とOwner/解放の移設先試験 | create/poll/claim/lease/release、取消途中の所有権。単体試験を実LARM成功とは扱わない |
| ASR | `bun test tests/ambient-voice-session.test.tsx tests/voice-capture-races.test.ts tests/voice-asr-packet-sender.test.ts`、`cargo test --manifest-path src-tauri/Cargo.toml voice::streaming_asr` | 既存の発話・競合・順序ケースを新入口へ移す。旧module削除でフィルタが0件にならないよう回帰module/コマンドの実行対象を明示して維持 |
| 初回応答 | `bun run quality:check`、macOSで `bun run desktop:smoke` | snapshotとprimary conversationをロード。品質評価を新入口へ移し、Memory/World未接続と通常回答成功を別評価する |
| TTS | 現行 `tests/streaming-speech.test.ts` と `voice::streaming_tts` の回帰を新Speakingへ移す | 句順、早期合成、取消、遅着音声、二重最終イベント、共有通知、forget時の発声抑止 |
| IPC/UI | `bun run typecheck`、`bun run ipc:generate`、`bun run ipc:check` | command_registry、Rust契約、TS生成物、fixtureを同時更新。生成物だけ手修正しない |
| 全体 | `bun run check`。完了時はプロジェクトの整形/lintを含む `bun run check:local` | coding/履歴/設定/管理機能の共有回帰。関連変更単位の検証後、最終段階で全体を確認 |

新実装と無関係な既存失敗は根拠付きで分離するが、失敗・0件・skipを成功と報告しない。

削除専用の確認も追加する。

- 新coreのbuild依存と新入口の呼出しが旧Runtime、Memory、Worldを含まない。
- 旧IPC名・module登録・env分岐・起動復旧を検索し、残す例外は履歴互換/他用途/試験fixtureごとに理由を記録する。
- 実機でASR final一件から仕事・LLM要求・最終message・音声再生が重複しない。停止後の遅着出力を採用しない。
- `SAAA_CONVERSATION_REASONING_MODE`、`SAAA_BUTLER_CONTINUATION`等で旧会話を再有効化できない。共有用途がある分岐は会話経路から切り離す。
- 削除ファイルに対応するsize/clippy baseline、IPC fixture、診断、監査表示を整理する。実測していない性能向上を記録しない。
- Runtime/Voice READMEと旧実装計画の「既存turnを必ず通す」指示を後継設計への参照へ改訂する。コンセプトと過去の障害証拠は残す。

## 9. 差戻しと未確認事項

実装開始前に、ユーザーの既存変更を含む復元可能なGit状態を保存する。実装・削除・freeze変更は段階別に差戻せる単位にする。既存の稼働プロセスを把握し、新旧を同時に動かして同じDB・マイク・leaseを所有させない。

差戻しではコードと対応するschema互換性を先に確認する。**本番DB全体を古いコピーで上書きし、その間に増えた会話を失う復元はしない。** 隔離試験DBはコピーから再作成可能とし、本番切替後は新旧データを保持したまま戻れる追加型migrationを優先する。戻す前に新sessionの停止・資源cleanupを確認する。取消不可の遠隔処理が残れば、その責任を明示する。

未確認のものは削除実装で解消する。

- 条件付きfeature、診断binary、生成コードを含む全buildでの参照残り。現時点の検索は完全なdead-code証明ではない。
- Memory/Worldを完全に切ったhostで、起動時のsnapshot取得やworkerが暗黙に資源確保しないこと。
- 新Resourcesが5役割を正式に確保できることと、Qwenをornithの冷間待ちから独立させられるLARM契約。これは削除だけでは保証できない。
- 新ASR/Speakingで実マイク・AEC・選択済みTTSを通した終了、停止、遅延。

この文書の作成時点では計画とinventoryだけを保存した。その後、ユーザーの指示で実機合格を待たずに旧実装の削除を開始した。現状の削除範囲と残存参照はGit差分で確認する。

運用ツールの会話累計：initial_instructions 1回、context_compile 5回、compile_eval 5回（今回の評価を含む）。
