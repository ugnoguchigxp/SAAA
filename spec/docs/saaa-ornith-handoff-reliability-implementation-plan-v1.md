# SAAA：Qwen → ornith 引継ぎ信頼性の実装計画 v1

作成日: 2026-09-26  
状態: 実装中。H1/H2のSAAA側対策を実装し、H4/H5は実LARMのprobe待ちとLARM daemon実装へのアクセスが残る。  
根拠: [handoff待機の調査](../evidence/handoff-investigation-20260926/REPORT.md)、[Jarvis 5 Provider実装計画](saaa-jarvis-five-provider-implementation-plan-v1.md)、[Runtime契約](saaa-jarvis-five-provider-runtime-contract.md)。

## 1. 目的と受入の境界

Qwenが `handoff` を選んだ後、ornithの接続準備・実行枠待ち・生成を区別し、ユーザーに長時間無応答のまま待たせない。同時に、実行枠を得られる条件では、最終回答を会話本文と音声まで届ける。Qwenの独立した初回応答、LARMの正式なAgent Connection契約、Ownerのcleanup責任、出力後の無条件再生成禁止は維持する。

この計画は既存のJ2b/J3/J5にまたがる補修単位H0〜H5として実施する。**接続待ちを短く打ち切るだけでは「ornithの回答が返る」の合格にしない。** 実LARMでの正常完走をH5とJ5の必須条件にする。

| ケース | 必須の結果 |
| --- | --- |
| Qwenだけで確実に答えられる入力 | ornith接続の準備・混雑に依存せず答える。分類評価は既存のJE基準を使う |
| 正当なhandoffで接続または実行枠が混雑 | 段階に沿った状態を表示する。暫定25秒の短い対話期限内に、生成が始まらなければ理由と再試行操作を提示して元runを終える。値はH0で実測して固定する |
| 準備済み接続と空いた実行枠 | 同じ入力からornithの生成、最終message保存、UI表示、TTS終了まで到達する |
| 期限・取消後の遅いready/出力 | 終了済みrunへ追記せず、二重生成・二重発声しない。Ownerが接続のrelease/cleanupを完了する |

本番のProvider設定を置換せず、移行・実験は隔離DBで行う。コード上の模擬試験やLARMのhealth、claim成功のみを最終回答の受入証拠としない。

## 2. 確認済みの原因と設計上の分岐

元run `run_f6cc9796-f0c0-4008-ac9c-cee782452727` では、LARMの資源allocation自体はreadyになった一方、別の約176秒のornith要求が実行枠を占め、semantic probeが99回 `probe_busy` になった。SAAAはclaim前の接続待ちで止まり、ユーザーが約114秒で取消した。ornithの生成要求は送られていない。

隔離実測では、120秒のstep期限まで接続待ちを続けて定型期限応答を保存した試行、接続に118秒を使い生成に約2秒しか残らなかった試行、245秒でclaimした後に生成キューが120秒でtimeoutした試行がある。従って、次の両方が必要である。

1. **SAAAの対話期限と結果表現を直す。** これはSAAAだけで先行実装できる。
2. **LARMのprobeと生成キューに、対話要求が進める条件を作る。** これはLARMとの契約・実装・負荷試験が必要。接続の先行確保だけで済ませない。

Qwenより先に現在の `SAAA` 5 Provider接続を一律claimする案は採用しない。現行の遅延claimはQwenの独立応答を守る目的があり、先行claim中にQwenが遅くなった過去の観測もある。LARM側でQwenとornithの資源を分けるか、準備済み接続のbackchannelをQwenが安全に使える契約ができるまで、この競合を残す変更を本番へ入れない。

## 3. 外部に見せる状態と期限の契約

### 3.1 状態

runと会話Ownerの状態を別に持つ。handoffは `frontend_reply` → `connection_wait` → `claim_ready` → `generation_queue` → `generating` → `answer_delivered` の到達段階を持ち、各段階から `user_cancelled`、`preparation_deferred`、`capacity_deferred`、`provider_failed` へ終端できる。これらは既存 `runtime_runs.status` の値ではなく、handoff固有の段階・結果である。Ownerはrun終了後も明示した上限内で準備を続けられるが、終了済みrunを再開しない。

LARMが現在公開していない細かい `probe_busy` / queue理由は、SAAA側で推測して表示しない。LARMから区別できるまでは「回答用の接続を準備中」「接続済み・実行開始待ち」を使う。`claim ready` は生成開始でも回答成功でもない。

「少し考えます。」は接続待ちと生成中を同じ状態として扱うため、handoffの固定文面を段階に合う短文へ変更する。たとえば接続待ちは「回答用の接続を準備しています。」、生成開始後は「回答を作成しています。」とする。10秒ごとの「はい。」の反復で進行を表現しない。音声の状態通知はTTS ownerを通し、先の通知が残る場合は重ねない。

### 3.2 期限

root・step・対話応答期限を絶対時刻として一度確定し、接続、claim、lease、生成、AllocationLost再試行へ同じ残余時間を渡す。DBの `rr_roots.deadline_at_ms` より後のProvider I/Oを始めない。現行の `role_dispatch.rs` が期間値をrouteへ渡してから `now + duration` を作る箇所を改める。

H0の試験値は、状態通知5〜10秒、生成が始まらない場合の対話期限25秒、最終生成へ進むための最低残余30秒とする。これらは製品の確定値ではない。対話期限が来たら説明文と再試行操作を保存し、元runを終える。`runtime_runs.status` は既存の終端値を使い、別のhandoff結果を `preparation_deferred` または `capacity_deferred` として記録する。接続が期限直前にreadyになっても、最低残余に満たない場合は生成要求を送らない。内部期限、ユーザー取消、モデル回答成功は別の結果として記録する。

ユーザーが再試行を選んだ場合は、保存済みの入力message IDを参照し、新しいrun/rootと新しい期限を作る。ASRが不完全な場合に同じ誤認識を無条件で再送しないよう、認識された本文を確認・編集できる。元runの後付け回答や自動二重実行はしない。J3の永続仕事キューが完成するまで、準備完了だけを理由に元の依頼を自動再実行しない。

### 3.3 監査と相関

credential、音声、生の会話本文を記録せず、`runId` → `ownerGeneration` → `agentConnectionId` → `allocationId` → `providerRequestId` を追跡できる最小限のIDと時刻を保存する。create受理、pollの**状態変化**、claim/health、lease、生成queue、最初の出力、release、cleanup保留を段階イベントにする。取消時にも開始済み段階のterminalを記録する。結果は `answer_success`、`preparation_deferred`、`capacity_deferred`、`user_cancelled`、`provider_error` を区別し、定型期限文をモデル回答成功として集計しない。H0で既存 `rr_events` と監査から参照できる追加の結果記録を定め、既存 `runtime_runs.status` のCHECK制約を無理に拡張しない。隔離DBでmigration・旧データ読取り・再起動復元を検証する。

## 4. 実装単位

| 単位 | 主な成果と変更箇所 | 依存・規模 | 完了条件 |
| --- | --- | --- | --- |
| H0：契約と基準 | 本書の期限値、LARM側の必要API、混雑の再現fixtureを固定。現行Qwen p50/p95と接続・queue時間を測る | 最初。小、外部依存高 | LARMが実際に公開できる状態・IDを確認。API拡張不能ならH4の代替分岐を決定 |
| H1：段階・監査 | `larm-session`、`larm_voice/mod.rs`、`providers/stream/larm_voice.rs`、Runtime監査に相関IDと全stageのterminalを追加 | H0後。中 | 元run相当の取消で接続/claim/生成の到達段階をDBだけで判定でき、credential/本文を漏らさない |
| H2：対話期限とUI | `conversation_provider_route.d/01.rs`、`role_dispatch.rs`、会話UIで状態通知、短い期限、結果分類、保存済み入力の再試行操作を追加 | H1の状態契約に依存。大、凍結高 | cold/busy時に無応答が対話期限を超えず、元runの終端・説明・再試行が一度だけ届く。root残余10秒の試験も通る |
| H3：Owner準備 | 現行の遅延claimを基準に、会話Ownerのバックグラウンド準備、寿命、世代、取消、cleanupを明示。先行確保は隔離モードから評価 | H0・H1後。中〜大 | Qwen即応を劣化させず、準備済みOwnerを再試行が再利用できる。会話終了・切替・アプリ再起動で接続を漏らさない |
| H4：LARM混雑契約 | semantic probeの飢餓対策、provider単位の準備/claimまたは同等の隔離、生成queueの状態・待機上限・優先制御をLARM側で実装 | H0でAPI仕様確定後。大、外部依存高 | 長い既存要求があってもprobeと対話生成の受入条件が測定値で成立。claim成功だけを合格にしない |
| H5：実LAN受入 | 隔離DBと明示モードでhandoffから最終UI/TTSまで試す。混雑・取消・再試行・復旧を実測 | H2〜H4後。中 | 正常なornith回答が最終messageとして保存され、表示・読み上げまで完走。失敗時も期限内に明示され、二重回答0 |

H1/H2はLARMの新APIを待たずに既存の状態粒度で進められる。H3の先行確保とH4は、Qwen経路との競合測定を通るまで通常入口へ切り替えない。H2の短い期限を入れた時点では「無応答」の改善であり、「ornith回答到達」の完了ではない。

相対的なリスクはH2とH4が最大である。H2は初回応答の凍結経路、Role Routingのactive root、結果保存・再試行を横断する。H4は別のLARM実装と共有GPUに依存し、SAAA側だけでは完了できない。H1は観測主体で低〜中リスク、H3はOwner寿命とQwen競合が中〜高リスク、H5は実機の混雑条件を再現できるかが主要リスクである。規模の目安はH0が小、H1とH5が中、H2〜H4が大とし、H0でLARM変更範囲が確定するまで日数の固定見積もりは出さない。

## 5. LARMとの契約変更案と判断

LARM側へは次を要求案として渡す。既存APIにこれらのフィールドがあると仮定しない。

1. `SAAA`全5 Providerの一括semantic readyを待たず、必要なProvider、特に `llm` の準備とclaimを表現できること。Qwen用backchannelの独立要求を止めない。API形式はLARM側と決める。
2. `allocation ready`、semantic probe待ち、claim可能、claim済み、生成queue待ち、実生成中を別状態として返すこと。busy理由・queue時間の上限を示し、credentialなしで読める必要最小限の状態を定める。
3. 同一runtime/release/modelの直近の有効なsemantic成功を再利用できるか検証する。cacheの失効条件をrelease・モデル更新・失敗に結び付ける。cacheだけで生成枠の空きを保証した扱いにしない。
4. 既存の長時間要求でprobeが飢餓にならない公平性と、claim後の対話要求が無制限にqueueへ置かれない実行枠管理を検証する。並列数を増やす場合はGPUメモリ、Qwenの応答、ornithの品質と速度を測る。

単一のornith枠を176秒の要求が占有する条件では、SAAAの待機ロジックだけで短時間のモデル回答を保証できない。LARMが公平な実行枠または妥当な待機上限を提供できない場合は、J5を保留し、長い要求の分割、別枠/別worker、Provider変更を設計分岐として比較する。計画の対象外だった複数workerも、根本条件が成立しなければ再審査する。

## 6. 回帰と実機受入

### 決定的試験

- `larm-session` のcreate/poll/claim/release: pending、probe busy、ready、claim拒否、health stale、release失敗、取消の各境界。既存と新APIの互換を確認する。
- Butler/Role Routing: 64/118/120/245/267/300秒の仮想時計、root残り10秒での次step、接続が期限直前にready、AllocationLostの1回再試行、内側timeoutと2秒tickの順序逆転。期限後の送信・保存・発声を0件にする。
- Owner: 複数turnの同時利用、初期化中のrun取消と会話終了の違い、古い世代のready、cleanup/release失敗、アプリ再起動。Agent Connectionの二重createとlease喪失を0件にする。
- 表示と保存: 各終端理由で説明文とイベントが一度だけ届く。`runtime_runs`、`rr_roots`、provider sessionの結果が同じ意味になり、モデル回答成功の件数に期限文を含めない。
- Qwen: H3/H4前後で冷間・温間・ornith busy時の初回本文/可聴開始を比較し、現行J0の目標を満たす。初回応答を接続待ちでブロックしない。

基礎コマンドは `cargo test --manifest-path crates/larm-session/Cargo.toml`、`cargo test --manifest-path src-tauri/Cargo.toml butler_route_tests`、`bun test tests/larm-voice-owner.test.ts tests/larm-voice-drain.test.ts tests/larm-voice-lifetime.test.tsx` とする。追加fixtureは上記の到達段階と失敗分類を実際に検証し、実装をなぞるだけの試験にしない。

### 実LAN受入

隔離DBと実際のLARM APIを使い、次を同じ相関IDで記録する。

1. 空いた枠: Qwen handoff → Agent Connection create/poll/claim → ornith生成開始 →最終message→UI表示→TTS終了。
2. semantic probe busy: Qwen初回応答は独立、短い対話期限で理由を表示、元runに遅い回答を付けない。Ownerを継続するモードでは準備完了後の明示的再試行を確認。
3. claim後queue busy: 接続成功だけで成功扱いにせず、実生成開始と期限内の最終回答、またはcapacity理由の終端を確認。
4. 生成後・出力後の取消: 未発声本文の停止、release=204またはcleanup保留、再試行時の二重回答0。

H5完了には少なくとも正常完走1件と、busy条件の再現試験が必要。正常完走は同時実行の単発プローブ、DBの `completed`、定型期限応答、mockされたProvider回答で代用しない。p95は反復測定から算出し、サンプル不足なら未達・未測定と記す。

## 7. 凍結、切替、戻し方

初回応答の凍結対象に触れるH2/H3は、`bun run quality:check` とmacOSの `bun run desktop:smoke`（snapshot・primary conversationをロード）を通し、理由付き `bun run freeze:accept:initial-response --reason "..."` 後に `bun run freeze:check` を通す。ASRの凍結対象を変更する場合だけ、AGENTS.md指定のBun 3試験とRust `voice::streaming_asr` を通し、ASR domainのみ理由付きでacceptする。凍結外の変更を一緒にacceptしない。

最初は隔離DBと明示モードで新しい待機契約を有効にし、本番保存済み設定をリセットしない。切替後の回帰では新規入力のみ旧経路へ戻し、既にLARMへ送った要求やTool操作を旧経路で自動再送しない。Ownerの接続はreleaseを確認し、失敗した場合はcleanup責任とlease keyを保持する。

## 8. 停止・再設計の判断

- H0でLARMからprobe busyと生成queueを区別する手段が得られなければ、SAAA UIに推定の詳細状態を出さずH4のAPI契約を先に詰める。
- H3の先行確保でQwenの即応が有意に悪化すれば通常入口へ入れず、Provider単位claimまたは別枠へ設計を戻す。
- H4後も176秒級の独占下で対話生成の実行枠が得られなければ、短い期限の状態通知だけを提供し、J5の「最終回答到達」を合格にしない。別worker等の構成を比較する。
- H5で最終message、UI、TTSのどれかが欠けたら、接続改善だけでは完了としない。

## 9. 2026-09-26 実装記録

- H1（一部）: SAAA側の接続・lease・生成要求の開始と終端を監査し、claim後のAgent Connection IDとallocation IDをcredentialなしで相関できるようにした。取消も接続・leaseの終端として残す。create受理とpoll状態変化のID付き監査は未実装。
- H2（一部）: voice handoffの接続待ちを暫定25秒、生成用残余を最低30秒とした。rootのDB期限で各role stepの予算を切り、AllocationLost再試行に残余だけを渡す。準備未了は `preparation-deferred` 監査イベントと明示メッセージで終了する。UIは認識済み入力をcomposerへ戻して編集後に新しい入力として再試行できる。Qwenのhandoff文面を接続準備中に変更し、繰り返しの音声fillerを停止した。provider sessionの既存CHECK制約には `timeout` を保存し、handoff固有の結果は監査に保存する。claim後の生成queue待ちに同じ短い期限を適用するには、LARMの開始状態契約が必要。
- H3: 既存Ownerの遅延claimと、callerが期限を迎えても準備とcleanupの責任を保持するworkerを維持した。通常入口での全Provider先行claimは有効化していない。
- 回帰: `bun run quality:check`、`bun run desktop:smoke`（snapshotとprimary conversationを含む）、`bun run freeze:check`、`cargo test --manifest-path crates/larm-session/Cargo.toml`、LARM ownerのBun 11件が通過。混雑fixtureでは25秒後に明示メッセージを保存し、ornith生成リクエストは0件だった。既存の共有LARM世界試験2件も再実行して通過。
- 実LAN: LARM APIでcatalog 200、Agent Connection作成成功。41.1秒後も `probing` でclaimに至らず、接続は解放した。これを最終回答成功の証拠にはしない。H4のdaemon側改修とH5の正常完走・UI/TTS確認は未了。
