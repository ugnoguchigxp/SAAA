# SAAA Mission Pilot — pi実行インターフェース実装計画

2026-09-13追記: 直近の実装範囲は[SAAA → pi ツール実装計画](saaa-pi-tools-implementation-plan.md)を正本とする。本書は将来のMission Pilotまで含む参考計画。Codex SDKを暫定既定とする後述の案は採用せず、SAAAの既存Qwenからpiへ接続する。

状態: PI-00接続実証を実装・検証済み。2026-09-13。Mission機能・NightWorkers廃止の完了を示さない。実施結果は第14節。

## 1. 採用する方向と到達点

SAAAがユーザーの目的を受け取り、piへ実装を依頼し、成果を評価して追加依頼または完了報告を行う。piは作業中だけ子プロセスとして起動し、結果回収後に終了する。次の依頼では同じ永続sessionを開く。pi本体をfork・改修せず、SAAA側にRustの接続層と最小のMission制御を実装する。

初期の体験は「一つの明示Missionを、一つのローカルGit作業領域で、複数回の依頼により完了させる」。会話画面を離れてもSAAAプロセスが動いていれば作業を続け、SAAA終了時は子プロセスを停止する。再起動後に同じsessionと成果へ戻れることを保証対象とする。SAAA停止中の無人継続は含めない。

判断例: ユーザーが機能追加を依頼 → SAAAが目的と受入条件を確定 → piが編集・テスト → SAAAが未検証の条件を発見 → 同じsessionへ追加依頼 → 現在の差分と検証結果を確認 → ユーザーへ成果を提示。

まず本計画の接続実証に投資を集中する。NightWorkersの代替判定は第12節の受入結果で行い、今回のcloneや計画作成だけでは既存機能を撤去しない。

## 2. 調査した実装と保証範囲

ローカルclone: `/Users/y.noguchi/Code/pi`。取得元: `https://github.com/badlogic/pi-mono.git`。

調査HEAD: `71dca871bc80b6bc97be37f0ca3189399d651fff`。packageの名前は `@earendil-works/pi-coding-agent`、記載versionは `0.85.1`。これはcloneしたソースの識別であり、同じversion表記の配布物との一致やリリース済みを保証しない。実装開始時に配布物のversion・実体・由来を照合して固定する。実行時にこのcloneを参照する設計にはしない。

| 確認事項 | ローカル根拠 | 計画への反映 |
| --- | --- | --- |
| `--session`で既存sessionを開く | [CLI main](https://github.com/badlogic/pi-mono/blob/71dca871bc80b6bc97be37f0ca3189399d651fff/packages/coding-agent/src/main.ts)、[SessionManager](https://github.com/badlogic/pi-mono/blob/71dca871bc80b6bc97be37f0ca3189399d651fff/packages/coding-agent/src/core/session-manager.ts) | SAAAが絶対pathを保持。最新sessionの自動選択を使わない |
| RPCのprompt受付は非同期実行の完了ではない | [RPC実装](https://github.com/badlogic/pi-mono/blob/71dca871bc80b6bc97be37f0ca3189399d651fff/packages/coding-agent/src/modes/rpc/rpc-mode.ts)、[受付テスト](https://github.com/badlogic/pi-mono/blob/71dca871bc80b6bc97be37f0ca3189399d651fff/packages/coding-agent/test/rpc-prompt-response-semantics.test.ts) | accepted、実行終了、Mission達成を分ける |
| `agent_settled`はretry・queue処理後の区切り | [AgentSession](https://github.com/badlogic/pi-mono/blob/71dca871bc80b6bc97be37f0ca3189399d651fff/packages/coding-agent/src/core/agent-session.ts)、[settledテスト](https://github.com/badlogic/pi-mono/blob/71dca871bc80b6bc97be37f0ca3189399d651fff/packages/coding-agent/test/suite/regressions/6363-agent-settled-event.test.ts) | `agent_end`だけで結果確定しない |
| `get_entries(since)`はentry ID以降とleaf IDを返す | [RPC契約](https://github.com/badlogic/pi-mono/blob/71dca871bc80b6bc97be37f0ca3189399d651fff/packages/coding-agent/src/modes/rpc/rpc-types.ts)、RPC実装 | 再接続時の履歴照合に使う。イベントの全量replay APIではない |
| `get_entries`にlimitはなく、sinceは欠落時にerror | RPC実装 | 応答上限と、大きなsessionのファイル読取経路が必要 |
| `steer`と`follow_up`の待ち行列はメモリ内 | AgentSessionのqueue処理 | 次のMission依頼はSAAAで保存し、pi queueを永続Task queueにしない |
| `abort`はidle待ち。queueを残すと続行し得る | AgentSession、[RPC文書](https://github.com/badlogic/pi-mono/blob/71dca871bc80b6bc97be37f0ca3189399d651fff/packages/coding-agent/docs/rpc.md) | 停止時は送信を凍結し、clear_queue → abortを順に実行 |
| stdin EOFでshutdown | RPC実装 | 作業中はstdinを保持し、結果回収後に閉じる |
| 新規sessionの初回保存はassistant messageまで遅延 | SessionManagerの`_persist` | 依頼は送信前にSAAAへ保存。ファイル不在を未実行の証拠にしない |
| 読込時に不正JSON行を読み飛ばし、末尾改行を補う場合がある | SessionManagerの`loadEntriesFromFile` | 異常終了後は起動前にSAAAが読み取り専用で整合性を検査する |
| JSON単発実行は終了コードだけではmodel errorを判別できない | [print実装](https://github.com/badlogic/pi-mono/blob/71dca871bc80b6bc97be37f0ca3189399d651fff/packages/coding-agent/src/modes/print-mode.ts) | 最終messageとtool結果を解析する |
| project trustは設定・extensionの読込許可 | [project trust](https://github.com/badlogic/pi-mono/blob/71dca871bc80b6bc97be37f0ca3189399d651fff/packages/coding-agent/src/core/project-trust.ts)、[README](https://github.com/badlogic/pi-mono/blob/71dca871bc80b6bc97be37f0ca3189399d651fff/packages/coding-agent/README.md) | `--approve`を操作権限やsandboxと解釈しない |

以上はコードと既存テストの読解結果。piの依存インストール、build、既存テスト、実モデルのcanaryは今回実施していない。Web調査時のスニペットより、このHEADの実装を優先する。

## 3. 既存SAAAとの責務分担

| 領域 | 所有者 |
| --- | --- |
| ユーザー原文・訂正 | SAAAの既存conversation_messages |
| 現在の目的・制約・判断の継続 | Personal State。未実装時はMissionに明示保存した値で動く |
| Missionの依頼版、委任、予算、次action、成果の採否 | SAAA Mission制御 |
| piプロセス、依頼の配送、session対応、回復 | SAAA pi Adapter |
| コード調査・編集・tool実行・局所的な計画・compaction | pi |
| pi会話・tool結果の正本 | pi session JSONL |
| 実際の変更と検証対象 | 対象Git作業領域と、その版に結び付いた証跡 |
| 再利用知識 | 既存ContextStill。今回新規保存経路は追加しない |

[Personal Stateロードマップ](personal-state-architecture-roadmap.md)の「Runtime状態の正本を重複させない」「外部agentへContext Packを渡す」を継承する。[P1計画](personal-state-phase-1-plan.md)のTaskInput / TaskUpdateと、task/run ID、request revision、source refsを合わせる。P1の完成を待たずにAdapterを試せるよう、Personal State投影を任意の入力portにする。20M KVはSAAAの判断用Contextであり、piへの共有・全量転送は行わない。

[現行全体計画](plan.html)はMission PilotをNightWorkersへ委譲している。本計画は、そのうちpi接続を選んだMissionについてSAAAが依頼・評価・次actionを所有する新しい経路の提案である。二つのPilotが同じMissionを同時に制御しない。実装開始時に全体計画の外部責務とMVP 5の記述を本経路と整合させる。NightWorkers経路の削除や全面移行は含めない。

### 現行実装から再利用するものと追加するもの

- `src-tauri/src/persistence/sqlite/writer.rs`（SqliteWriter）とreader poolを再利用し、別DBを作らない。transactionは短くし、子プロセスやモデルの待機中にwriterを保持しない。
- `src-tauri/src/persistence/schema.rs`（runtime_runs）は会話等の実行台帳であり、Missionの依頼・評価台帳ではない。Mission実行は新しい専用table群に一度だけ保存する。会話runとは参照で結び、二重の実行状態を作らない。
- `src-tauri/src/runtime/codex_process.rs`（既存Codex接続）はread-onlyで、既存Supervisorには最大300秒の制約がある。piをこの分岐へ単純追加して、編集可能な長時間実行に流用しない。プロセス管理の技法を参照し、新しいprofileを分離する。
- `services/reasoning-mcp/src/lib.rs`（既存reasoning MCP）はstatelessで、下流tool実行を持たない。ここへMission台帳やpi起動を埋め込まない。Mission ControllerはSAAA側に置き、既存推論接続へ型付き判断を依頼するportを実装する。
- `src-tauri/src/runtime/event_hub.rs`（既存イベント配信）は会話・音声向けで、UI切断を失敗として扱う経路がある。Missionの永続化をUIより先に行い、UI購読解除でpiを止めない。再購読は保存済みevent sequenceから追いつく。
- IPCは`src-tauri/src/ipc_contract/bindings.rs`（既存Rust→TypeScript生成）へ追加し、生成ファイルを手編集しない。

## 4. 最初に作る範囲

含む: 明示Mission作成、単一実行、同一session再開、結果回収、SAAAによる評価と追加依頼、ユーザーへの質問、訂正・取消、異常終了の照合、予算停止、最小UI、決定的試験とlive canary。

初期は同時実行数1とする。複数Missionの記録は保持できるが、自動の優先順位queueは作らず、実行枠が空かない依頼にはbusyを返す。会話は並行して続けられる。

含めない: pi本体改修、Node SDKの組込、JSONとRPCの二重実装、OpenCode Adapter、汎用agent framework、分散worker、常駐daemon、自律Goal生成、NightWorkersの計画UI・review engineの移植、merge/push/deployの自動化、piへのSAAA KV共有。

将来のGoal / Policy全体がなくても、当該Missionに明示された委任と受入条件で成立させる。推測から新たな権限を生成しない。初期releaseはfeature flagをdefault OFFとし、設定済み環境で明示Missionだけを実行する。

## 5. 外部公開するSAAA側契約

以下は新設するapplication commandの案。UIとMission Controllerは同じcommandを利用する。モデルにOS pathやpi任意RPCを直接生成させない。hostがIDから実体を解決する。

| Command | 入力 | 出力・意味 |
| --- | --- | --- |
| create_mission | 原文source refs、目的、受入条件、workspace参照、委任参照、予算、idempotency key | mission IDとrevision。作成だけではpiを起動しない |
| submit_mission_request | mission ID、expected revision、依頼本文、context refs、idempotency key | 永続request/run ID、queued-local。受付後の起動は非同期 |
| inspect_mission | mission ID、after event sequence、limit | 現在状態、保存済みevent、成果・質問参照、次cursor |
| revise_mission | mission ID、expected revision、訂正source refs、新しい有効条件 | 新revision。実行中なら旧結果の採用を無効化し中断を開始 |
| cancel_mission | mission ID、expected revision、理由source | cancel_requested。停止確認は別event |
| resume_mission | mission ID、expected revision、回復評価参照 | 既存sessionの照合後に新requestを発行。元の不明な依頼を再送しない |
| answer_mission_question | mission ID、question ID、expected revision、回答source refs | 回答を保存。新しいrequestで同じsessionへ渡す |

requestは原文を参照し、piに渡した最終payloadをimmutableな実行入力として保存する。会話本文の第二の編集可能な正本にはしない。payloadにはmission ID / request ID / revision、目的、受入条件、現在有効な制約、訂正、必要な出典、完了報告に求める内容を含める。20M全体や関係のない個人情報を投入しない。

冪等キーはSAAAへのcommand重複を防ぐ。piのRPC `id`は応答相関用であり、同じidのpromptを再送しても一度だけ実行される保証はない。

## 6. pi接続の実行手順

### 起動と依頼

1. Missionのrevision・委任・予算を検証し、requestと配送状態をtransactionで保存する。sessionとworkspaceの実行所有権を取得する。
2. 設定された実行ファイルをshellを介さずargvで起動する。基本形は `pi --mode rpc --session <absolute-session-path>`。cwdをworkspaceへ固定し、promptはstdinのJSON値として送る。
3. `get_state`の応答をready handshakeとして、session ID / path / modelを検査する。新規sessionなら対応を保存する。再開時に保存済みsessionが消えていたら、新規作成へ黙って進まない。既存headerのcwdも起動前に照合する。
4. 起動直後のentry/leafを記録してrequestの開始境界にする。payloadにはhost生成のrequest識別子を固定位置に入れる。照合にはuser entryのrole、境界、payload digestを使い、assistantの引用文を配送証拠にしない。
5. `prompt`を一件送信する。responseを待つ前にイベントが来ても処理できるようreaderを先に起動する。`success=true`はaccepted、falseはrejectedとして保存する。
6. eventを解析し、SAAAのrun ID・process generation・sequenceを付ける。piの通常eventにはrequest IDがないため、同一processへ複数の実装依頼を同時送信しない。

初期の追加依頼はpiの`follow_up` queueを使わず、SAAAが結果評価後に永続化して新しいprocessへ送信する。これにより一つのrunと一つの依頼を対応付ける。

### 結果回収と終了

`agent_end`、最後のtext、`isStreaming=false`だけでは完了を決めない。通常経路は`agent_settled`を受信し、`get_state`と`get_entries(since)`で状態と今回のentryを回収する。最終assistant message、tool error、未解決tool call、compaction/retryの失敗を正規化する。古いsessionの最後の回答を今回の結果として返さない。

`stopReason=error/aborted`は失敗・中断、`length/toolUse/pending/deferred`や不明な値は完了候補にしない。`stop`もMission達成の証明ではなく評価待ちとする。途中のtool失敗から回復した場合は、その履歴を残したまま最終結果を評価する。

結果とentry参照を保存したらstdinを閉じる。stdout/stderrをdrainし、exitを回収してprocessをreapする。結果回収後も終了しなければ停止を段階的に強め、cleanup failureを別に保存する。最終message eventはpiのディスク保存より先に送られるので、event受信だけをsession永続化成功として扱わない。process終了後にsessionの該当entryと整合性を確認する。

RPCには一般的なprotocol version handshakeがない。対応version・binary fingerprintとfixtureで契約を固定し、必須commandが使えない場合はunsupportedとして起動を止める。自動updateや無条件fallbackはしない。

### 出力量と待機時間

LF単位でJSONLを復元し、UTF-8途中分割、CRLF、U+2028/U+2029を扱う。stdoutはprotocol、stderrは診断として独立してdrainする。不正な必須event・途中EOF・上限超過を成功に変換しない。

初期値案: 一行8 MiB、runの診断・イベント記録64 MiB、ready/受付待ち30秒、graceful停止10秒、SIGTERM後5秒。実行deadlineはMissionで明示し、初期UIの既定30分、最大4時間とする。無出力60秒等で長いテストを誤停止しない。UI deltaは間引けるが最終message、tool結果、配送・停止eventは間引かない。上限はPI-00で実測して調整し、変更理由を残す。

`get_entries(since)`はページングではなく「以降すべて」である。上限を超えるときはprocessを停止してからsession JSONLを読み取り専用でstream解析し、SAAAのinspect側でpagingする。壊れた行・不正parent・cycle・同一IDの内容変更を検出したらcorruptとして保留する。piのcompactionをSAAA側で再実装せず、現在branchの参照関係と証跡のみ検査する。

## 7. 配送・実行・成果の状態を分ける

| 状態軸 | 値の案 | 正本 |
| --- | --- | --- |
| Mission | draft / ready / running / evaluating / waiting_user / completed / cancelled / blocked | SAAA |
| 配送 | prepared / sending / accepted / rejected / unknown | SAAA request |
| pi run | starting / running / stopping / settled / failed / interrupted / outcome_unknown | SAAAの観測記録 |
| 成果 | unevaluated / accepted / needs_work / needs_user / insufficient_evidence / superseded | SAAA評価記録 |

`sending`以後に接続が切れた依頼はunknownとし、自動再送しない。SAAA内の一意制約だけでpiとのexactly-onceを保証しない。SAAA起動前に保存されたpreparedだけは、プロセスをまだ起動していないことが確定する場合に送信できる。

保存する最小table群は `missions`、`mission_requests`、`pi_session_bindings`、`pi_runs`、`mission_events`、`mission_evaluations`。名称は実装時の衝突を確認して確定する。必要な項目は次の通り。

- missions: source refs、目的・条件、revision、workspace/委任参照、deadline、最大追加依頼数、status。
- mission_requests: run ID、revision、payload/digest、idempotency key、配送状態、開始entry境界、原因となった評価/質問ID。
- pi_session_bindings: mission ID、session ID/path、cwd、binary/model/resource profile、最終照合entry/leaf、health。
- pi_runs: process generation、PIDと起動identity、開始/終了時刻、停止理由、結果参照、使用量、cleanup状態。PIDだけで既存processをkillしない。
- mission_events: run ID、単調sequence、event種別、entry/toolCall参照、必要なpayload、記録時刻。raw delta全量を長期正本にしない。
- mission_evaluations: request revision、結果digest、workspace状態digest、条件別評価と出典、採用action、モデル識別、時刻。

pi session全文はSQLiteへ複製しない。取得した証跡の抜粋・digestは派生記録として扱い、session削除・Mission削除時に関連出力と継続利用も失効させる。訂正は新依頼へ反映し、削除されたsourceを含むpi sessionは以後再開しない。必要なら有効sourceだけから新sessionを作る別actionにする。

## 8. 訂正・停止・再起動

初期の訂正は、revision更新 → 旧runの結果をsuperseded扱い → 新規送信凍結 → `clear_queue`応答 → `abort`応答 → 回収・終了 → 新条件を同じsessionへ依頼、の順とする。通常の取消では最後の再依頼を行わない。

`steer`は後段の任意改善とし、初期の必須条件にしない。steerは次のモデル境界で反映されるため、「送信したから既存のtool操作も止まった」と説明しない。追加する場合はrevisionごとの送信・反映証拠を管理する。

SAAA正常終了では新規受付を止め、上記停止を実行し、sessionを保存して閉じる。ウィンドウ非表示や会話切替は停止条件にしない。

SAAA異常終了後は旧processの所有identityと生存を確認する。同じpipeへの再接続はできない。所有する旧processが残っていれば停止・終了確認を行い、確認できない間は二重起動しない。PID再利用を排除できなければblockedで操作を求める。OSのprocess group管理は実装するが、detached childを含む全子孫停止の保証はPI-00/02の実測で確認する。

再開時は、session header・entry整合性、送信前境界、user entry、tool結果、現在のGit状態を照合する。配送済みか曖昧な依頼は「実施済みの変更と未完事項を確認する」という新requestとして回復評価に渡す。履歴がないことだけを根拠に、同じ副作用をやり直さない。再起動から処理途中のcommandを復元する保証は持たない。

既存の起動時`reconcile_interrupted_runs`は新しいMission台帳を更新しない。新しい回復処理を同じbootstrapへ追加し、旧conversation runの終了判定をMission達成判定へ流用しない。

## 9. SAAAがMission Pilotとして判断する部分

Mission Controllerは `dispatch / request_changes / ask_user / accept / block` の型付き判断を既存の推論経路へ依頼する。判断入力は現在の依頼版、ユーザー原文、受入条件、pi結果、証跡参照、残予算。モデルは次actionの候補と理由・条件別根拠を返し、hostはrevision・参照範囲・委任・予算を確認して採用する。

初期は固定の「Plan→Implementation→Test→Review」段階を再構築せず、目的に応じた依頼をモデルが作る。コードを書く内部手順はpiに任せる。piの自然文をkeywordで完了分類しない。schema不一致や推論失敗時は原文を保持してblocked/evaluation_unavailableとし、完了や再依頼を捏造しない。

acceptには全受入条件の評価と証跡参照を要求する。testのpassだけで意味的な達成を確定せず、SAAAが差分を確認する。逆にpiの「完了しました」だけでテスト済みと扱わない。検証が必要なのに証跡がない場合はinsufficient_evidenceを返す。

証跡はworkspaceのHEADだけでなくtracked変更とuntracked成果物を含む状態digestに結び付ける。検証後に変更が入った場合は評価を失効させる。piのcommand出力は実行観測であり独立検証ではない。初期canaryではSAAA側の限定した検証commandを同じworkspaceで実行し、exit codeと出力を記録してからacceptする。任意のshellをモデルから直接この検証経路へ渡さず、対象projectに登録した検証commandをargvとして実行する。

推論待ちの間も取消と訂正は処理する。遅れて返った旧revisionの評価を採用しない。追加依頼数は既定3回、最大数はMission設定に従う。変更なし・同一失敗の繰り返しはモデル評価に渡し、数値予算超過はhostが停止する。provider使用量・費用が不明ならunknownを表示し、厳密な金額上限を保証しない。

2B会話フロントは受付・進捗・質問の伝達を担う。27B等の評価担当を用いる場合も既存のProvider / LARM接続を参照し、pi認証とSAAA推論認証を混ぜない。上記Controllerは新規実装であり、既存reasoning MCPだけで自律反復が成立済みとは扱わない。

## 10. 作業領域・設定・UI

初期対象は、ユーザーが選んだローカルrepositoryの明示workspaceとする。専用worktreeがある場合はそれを利用し、人間との同時編集を避ける。worktree作成・merge・cleanupの汎用自動化は後回しにする。再開時に別directoryへ暗黙移動せず、移動・消失は設定修復として扱う。

pi実行path、確認version、session保存directory、provider/model、resource profileをSAAA設定へ保存する。session保存先はSAAA app data配下とし、調査cloneや一時directoryを本番session置場にしない。認証はpiの既存設定を利用し、秘密値をSAAA DB・argv・ログへ複製しない。

初期はglobal/project extensionやprompt template等の実行資源を列挙してprofileを固定する。暗黙のextensionから追加promptやsession切替が発生しない最小profileでPI-00を実施し、必要なAGENTS.md・skillは明示して加える。project trustの有無と実行されたresourceを診断に残す。

piは通常のローカルCLIとしてユーザー権限で動く。cwd指定、worktree、tool名制限、`--no-approve`はいずれもOS sandboxではない。初期の委任はこの実行形態と整合する信頼済みローカル作業に限定し、API上の権限確認がpi内部の任意shellにまで強制されると説明しない。workspace外書込やnetwork操作の強制隔離が必須となった場合は、既存OS/container実行profileの接続を独立した追加要件として評価する。pi本体改修を先に選ばない。

UIは既存会話内のMissionカードと設定項目を追加する。表示は目的、処理状態、最新結果、必要な判断、停止・再開・追加依頼、差分/証跡への参照。通常Chatへsession選択UIは戻さない。質問は型付きのSAAA評価結果として表示し、再起動後にも回答できるよう保存する。初期profileで不要なextension UI要求が来たらunsupportedとして安全に中断し、待ち続けない。RPC extension UIの全面再現は含めない。

## 11. 実装順序と配置

| 段階 | 実装・成果物 | 完了条件 |
| --- | --- | --- |
| PI-00 接続実証 | 配布物と調査HEADの照合、隔離された小repository、RPC契約fixture、結果レポート | 同一sessionで2回依頼し間にprocess終了。失敗と停止も識別できる |
| PI-01 永続契約 | Mission/request/session/run/event/evaluationの型・差分migration、application commands | revision・冪等・実行所有権・source失効をモデルなしで確認 |
| PI-02 pi Adapter | Rust process管理、JSONL parser、相関、結果回収、停止、再起動照合 | 通常/異常系fixtureと実CLIの制御試験が通る |
| PI-03 Pilotの反復 | 推論port、依頼生成、条件別評価、検証証跡、質問、追加依頼、予算 | 実モデルで依頼→評価→追加依頼→成果採用が通る |
| PI-04 会話と運用 | UI、Context投影、訂正・取消、診断、feature flag | 会話並行・UI切断・アプリ再起動で状態が混ざらない |
| PI-05 代替判定 | 複数Missionのcanary結果と制約一覧 | 第12節を満たしNightWorkersへの投資判断を記録 |

PI-00でsession再開や結果回収が成立しなければ、PI-01以降を広げず具体的な不足を評価する。依存installが必要な場合、pi側AGENTS.mdに従いlifecycle scriptを無条件実行しない。配布CLIで試せる場合はcloneのbuildを必須にしない。

配置案:

- `src-tauri/src/missions/`: contracts、application commands、repository、controller、evaluation、recovery、projection。Mission固有の意味判断と台帳を所有。
- `src-tauri/src/runtime/pi/`: process、protocol、event projector、session reader、capability probe。Missionの意味的な完了を判断しない。
- `src-tauri/src/persistence/schema.rs`とmigration: 既存writerへの差分追加。最新schemaから採番し過去migrationを改変しない。
- `src-tauri/src/lib.rs`、IPC契約/生成、既存settings: command登録・起動回復・shutdown・設定の最小接続。
- `src/features/missions/`、既存chat/settings: Missionカードとcommand client。実行状態の正本はRust側。
- `scripts/pi-interface-canary.ts`、`tests/fixtures/pi-rpc/`、Rust統合テスト: protocolとlive実証。上記pathは新設予定であり実装済みではない。

大量の既存未コミット変更がある。実装開始時にHEADとdirty一覧を記録し、同等のMission基盤が増えていれば再利用して表の重複を避ける。本計画作成時の配置から無条件に新規エンジンを増やさない。

## 12. 検証とNightWorkers代替の判定

### 決定的な受入試験

| ケース | 期待結果 |
| --- | --- |
| acceptedの後にmodel error | Missionをcompletedにしない |
| agent_end → retry → agent_settled | 評価はsettled後に一回だけ |
| 最終textが既存sessionにあるが今回の依頼は失敗 | 前回結果を採用しない |
| sending直後に接続断 | unknown、同じpromptの自動再送なし |
| 同一commandの再試行・同一sessionの二重起動 | 一件のrequest、二つ目のprocessを起動しない |
| get_entriesでsince欠落・巨大応答 | 完全性を偽らず停止後のstream照合またはblocked |
| session削除、ID/cwd変更、壊れたJSON、branch不整合 | 新規sessionへ黙って切り替えず診断する |
| 初回assistant前の強制終了 | ファイル不在でも未実行と断定しない |
| 訂正と評価の競合 | 新revisionが勝ち、旧評価から追加依頼しない |
| clear_queue/abort、停止応答なし、子process残存 | 停止を段階実行し、未確認状態をcancelled完了にしない |
| UI購読解除・再購読 | 実行継続、保存済みsequenceから表示復元 |
| 検証後のtracked/untracked変更 | 証跡を失効し再評価 |
| 推論失敗・不正schema・予算上限 | 内容を保存し停止。無制限の再依頼なし |
| 出力分割・巨大行・stderr大量出力 | parserの誤判定・deadlock・無制限メモリ増加なし |

fixtureはwire契約とプロセス挙動の検証であり、pi実物の互換性やcoding品質の証明には使わない。pi既存の受付、queue、settled、session保存テストを参考にし、必要なケースのみ実行する。live providerを暗黙に使う全suiteは実行しない。

実装時に追加するコマンド案（現在は未実装）:

```bash
cargo test --manifest-path src-tauri/Cargo.toml --test pi_interface_contract
cargo test --manifest-path src-tauri/Cargo.toml --test mission_recovery
bun test tests/mission-interface.test.ts tests/mission-ui.test.tsx
bun run pi:canary --profile fixture
bun run pi:canary --profile live --workspace <isolated-repository>
```

Rust/IPCを変更した段階では `bun run ipc:generate` → `bun run ipc:check`、各実装段階の統合時には `bun run check:local` と `bun run test:rust-packages` を実行する。S11t生成物を変更した場合は既存生成/check経路を利用する。既存変更由来の失敗はbaselineと比較し、本変更由来と分けて記録する。

### 実CLI・実モデルでの代替判定

小さな機能追加、既知の不具合修正、既存実装へのテスト追加の3種類を対象とする。各Missionで最低2依頼を同じsessionで行い、間にpiを終了する。少なくとも一件でSAAA再起動、一件で途中取消/回復、一件でユーザー訂正、一件で失敗からの追加依頼を含める。

記録項目: binary version/由来、model、session ID、process起動回数、request/entry対応、差分、検証commandとexit code、成果採用の根拠、介入回数、所要時間、使用量、未解決制約。fixtureとliveの結果を分ける。

代替採用条件は、全ケースで同じsessionを維持でき、重複依頼・誤った成功・作業混同がなく、最終受入条件を満たすこと。coding品質や介入量が用途に足りない場合は、まずmodelと依頼Contextの不足か、pi契約の不足かを切り分ける。NightWorkers比較が必要なら同じ対象・条件で別の隔離workspaceにて比較する。

- 全条件を満たす: SAAA＋pi接続へ集中し、NightWorkersの新規coding機能投資停止を確定する。既存資産の削除は別判断。
- Adapterの局所的不足: 接続部分を修正して該当ケースだけ再検証する。
- 標準RPC・CLI・extensionでは埋まらない必須要件: 最小の不足を示し、upstream修正、既存実行基盤の限定維持を比較する。直ちに独自coding loopへ戻らない。
- 厳密な副作用復旧、開発gate、常駐実行等が必須: 追加要件としてNightWorkersの該当資産を評価する。単なるsession継続要件と混同しない。

## 13. 本計画作成時点の結果

- piを指定された親directoryへcloneし、HEADとclean状態を確認した。
- RPC、単発実行、session保存・読込、project trust、関連テストとSAAAの接続候補を調査した。
- pi本体やSAAAのproduction codeは変更していない。依存導入、実CLI起動、live provider利用は未実施。
- 次の実装作業はPI-00の接続実証から開始する。PI-00の結果を本書に追記してから台帳とPilot実装へ進む。

## 14. PI-00実装結果（2026-09-13）

`@earendil-works/pi-coding-agent@0.85.1`を通常のnpm globalへインストールした。lifecycle scriptは無効化。調査用cloneは改修していない。SAAAの`bun run pi:canary`でPATH上のpiを利用でき、`SAAA_PI_BINARY`で実体の絶対pathを指定できる。versionが一致しなければ試験を中止する。

配布物のnpm integrityは `sha512-FGRN+OHbWaefBPGaTggAdLjrIHW+s2PzLyglz/5dfLzb9of7uuXMXYC0fJIeZTw+shS32o2cuQ9jF7YSDuL/oQ==`。実行したCLI実体のSHA-256は `e6d7fcf36a239cf3746e67ddf4222081ac01a601b85a3ee688bdfe9c161d754c`。調査HEADとのソース全体の同一性は保証せず、必要なRPC契約を配布物で実測した。

| 検証 | 結果 | 範囲 |
| --- | --- | --- |
| pi実CLI＋loopback応答fixture | 合格 | 4回の独立process起動、同一sessionの2回編集、履歴がproviderに届くこと、model error、clear_queue/abort、終了後session JSONL読取 |
| RPC parser試験 | 4件合格 | UTF-8の分割、Unicode改行文字、受付と終了の分離、切れたJSONと非zero終了の拒否 |
| Codex SDK＋gpt-5.6-luna実モデル | 合格 | 隔離Git workspaceで編集、SDK再生成、thread再開、前回だけで渡したmarkerによる再編集 |
| SAAA既存Rust coding経路＋Luna実モデル | 合格 | execute_turn、応答のDB保存、次runで保存済みthread IDを利用、履歴再開。既存read-only契約を維持 |

再実行コマンドは `bun run pi:canary`、`bun test tests/pi-interface.test.ts`、`bun run codex:canary`、`bun run coding:canary`。後二つは実モデルを使い、明示実行時だけ認証とnetworkを利用する。通常のtestでlive canaryは動かない。pi試験のJSON reportは出力された一時directory内にも保存する。

追加依頼に従い、SAAAの新規設定の既定modelを`gpt-5.6-luna`へ変更した。このMacの既存SAAA設定もwriter所有lock取得・旧documentのbackup後に、Codex有効・Luna・app-serverへ更新した。一般利用者の既存model選択をmigrationで上書きしない。

**接続方式の区別:** Codex SDKはインストール済みの`@openai/codex-sdk@0.144.4`を使い、既存ChatGPTログインで動作した。SAAAの既存production coding経路はRustからCodex app-serverを呼ぶ。名前は`codex-sdk`だがTypeScript SDKを呼ぶ経路ではない。SDKの編集試験合格を、SAAAの編集Mission実装済みという意味にはしない。[公式SDK文書](https://developers.openai.com/codex/sdk)でもSDKによるthread再開を確認した。

piのOpenAI Codex providerは別の認証・実行経路であり、Codex SDKを内包しない。現在piの認証は未設定。pi＋Lunaのlive試験は未実施で、利用にはpi側のOAuthログイン等が必要。SDKの認証tokenをpiへコピーしていない。

PI-01以降のMission台帳、書込委任用Adapter、評価反復、UI接続は未実装。追加要件の実行先は暫定的にCodex＋Lunaを既定とし、piを選択肢として残す。どちらをMissionの既定にするかに合わせ、第4節のNode SDK除外と第11節のAdapter配置を次段階の前に更新する。停止時の全子孫回収、異常終了後の二重起動防止、実Missionの受入試験も残るため、NightWorkersへの投資停止はまだ確定しない。
