# Personal World Model M2A 詳細作業カード

作成日: 2026-09-20。状態: 実装計画。全30枚（M2-00〜M2-29）。

[全体計画](saaa-personal-world-model-m2-plan.md)と[固定契約](saaa-personal-world-model-m2-execution-contract.md)を先に参照する。旧Dカードの続きとして一括実装せず、本書の一枚ずつを実行する。

## 実行規則

各カードの前提は直前カード完了と指定R節。出力は対象コード、具体期待値をassertする試験、進捗記録である。DTO→純粋処理→owner読取→Adapter→統合の順で、後続の未実装APIを製品経路へ接続しない。

標準は実装1〜3ファイル、テスト1〜2ファイル。module登録とimportは機械的追加として許可。実装5ファイル超または独立した二つの設計判断が必要なら、先に枝番へ分割する。カード単位でcommitやユーザー確認を強制しない。

対象外: 新しいSource正本・DB表・公開API・Tool・LLM prompt・Provider routing・通常Broker接続。private helper名と同一契約内の関数分割は担当者が決めてよい。上限、wire field、状態の意味、認可・期限、エラー、合格基準は変更しない。

テスト名は `m2_NN_説明`。core試験はcore/tests/world_m2_*.rs、adapter試験はworld/runtime_*_tests.rsへ責務ごとに分ける。既存v2_tests.rsへすべて追記しない。fixtureは合成Sourceと一時DBだけを使い、実会議・Provider・pi subprocess・外部サービスを起動しない。

## カード

coreは `crates/personal-state-core/src/world/`、adapterとworldは `src-tauri/src/memory/personal_state/world/`。meeting/codingは `src-tauri/src/meeting/` と `src-tauri/src/coding/`。

| ID | レイヤー・責務 | 許可変更先 | 入力契約 | 操作 | 具体的な合格条件 | lane |
| --- | --- | --- | --- | --- | --- | --- |
| M2-00 | 記録 | 既存v2結果・package.json・進捗文書 | 計画§3/8 | HEAD・差分・全G1〜G6をbaselineへ記録。再現しない旧ビルド不能を現行問題と扱わない | コマンド・件数・失敗原因・未実施を区別。既存失敗を消すための修正はしない | baseline |
| M2-01 | 既存試験 | world/v2_tests.rsのd41_v2_performance | 計画§6 | requested/createdを分離し、成功assert、warm-up5・30sample・nearest-rank p95を追加 | 未完成1000件を1000件の実測と表示しない。query Errを時間だけで成功扱いしない | adapter |
| M2-02 | core | core/runtime_frame.rs（新規） | R1 | RuntimeRef・StateView・Focus・WorldFrameのDTOだけ追加 | wire roundtrip、未知kind拒否、非許可field拒否。WorldSliceV2とv2 payloadは不変 | core |
| M2-03 | core | core/runtime_frame.rs | R1/R9 | normalize_runtime_refsと入力上限・ttl・ID検証を実装 | 重複排除後8件可/9件Limit、ttl0拒否、1001→1000、max_bytes=1を増やさない | core |
| M2-04 | core | core/runtime_frame.rs | R6/R10 | Frame stampのcanonical化、時刻を除くcontent digest、期限比較を実装 | 999/2000はExpired、1999は期限内。as_ofだけ変更は同digest、状態変更は別digest | core |
| M2-05 | adapter | adapter/runtime_scope.rs（新規） | R2 | authorize_frame_requestでrun・principal・policy・Project・epochを検査 | missing/ambiguous/別principal/旧policy/停止runはScopeDenied。resolveや登録SQLを呼ばない | adapter |
| M2-06 | adapter | adapter/runtime_scope.rs | R2 | authorize_runtime_refsで対象scopeとdirect linkを検査 | リンクなし/未登録/別Projectは拒否。無向・推移リンクだけでは通らない。全対象を検査 | adapter |
| M2-07 | meeting owner | meeting/types.rs・meeting/mod.rs | R3 | WorldMeetingSnapshotとMeetingRuntime::world_snapshotを追加 | 返却fieldはsession_id/stateだけ。token・本文・errorなし。前後でowner state不変 | meeting |
| M2-08 | adapter | adapter/runtime_meeting.rs（新規） | R3 | 指定ID一件のDB最小列読取と時刻parseを実装 | 行なし/discardedはUnavailable、不正時刻Corrupt、SELECTにtranscript/token列なし | adapter |
| M2-09 | core | core/runtime_frame.rs | R3 | meeting DB/live対応表とowner_digestの純粋合成を実装 | active+Active→running、active+Stopping→stopping、active+None→省略、終端＋別会議は終端 | core |
| M2-10 | coding owner | coding/world_snapshot.rs（新規）・coding/mod.rs | R4 | bounded run履歴確認、repository::authorize、最小列のsnapshotを追加 | 32履歴可/33省略、workspace/result全文なし、complete非booleanはnull | coding |
| M2-11 | adapter | adapter/runtime_scope.rs・新規adapter/runtime_coding.rs | R2/R4 | coding初回/現在/履歴SourceのProject・版・available・tombstoneを検査 | 別conversation/削除Source/未mappedでUnavailable。本文を返さず版tupleを保持 | adapter |
| M2-12 | core | core/runtime_frame.rs | R4/R7 | coding phaseとRuntime Focusを純粋変換 | running+acceptedだけactive、cancel_requestedはstopping、settledはterminal。Goal完了edge0件 | core |
| M2-13 | adapter | adapter/runtime_capacity.rs（新規） | R5 | Project投影100・全ledger2000のbounded preflightを実装 | 100可/101省略、2000可/2001省略。超過時store::load0回、既存履歴削除0件 | adapter |
| M2-14 | core | core/runtime_frame.rs | R7 | assemble_frame_partsでRuntime単位採否・JSON固定費・graph残予算を実装 | Runtime8以内、総Node30以内、1 byteエラー、条件/根拠の部分削除なし、Focus参照切れ0 | core |
| M2-15 | adapter | adapter/runtime_graph.rs（新規） | R5/R7 | 認可済み外側入力からActivateInputV2を構築して既存queryを一回呼ぶ | request/access/nowの不一致を作らず、観測配列は空。stale/pending/capacityはgraph None | adapter |
| M2-16 | adapter | adapter/runtime_frame.rs（新規） | R6 | 内部WorldFrameServiceとPreparedWorldFrame、OwnedFrameRequestを定義 | instance_idはサービス内で一度だけ生成。内部型をIPCやserde公開しない。独立DBなし | adapter |
| M2-17 | adapter | adapter/runtime_frame.rs | R6 | prepare_frameの事前Scope→live-before→DB snapshot→live-after→整形を接続 | 会議変化は一単位省略、DB/Mutexを同時保持しない、自動retry0回、書込み0件 | adapter |
| M2-18 | core | core/runtime_frame.rs | R6/R9 | stampの差分をCurrent/Changed/Expired等へ分類 | revision同じでもowner digest差はChanged、instance差はExpired、期限境界を正確に判定 | core |
| M2-19 | adapter | adapter/runtime_frame.rs | R6/R10 | revalidate_frameを現在DB/liveへ接続 | Goal期限切れ/Source削除/link削除を検出。古いFrameを書換えてCurrentにしない | adapter |
| M2-20 | adapter | adapter/evidence_eligibility.rs（新規）と既存typed_recall fixture試験 | R8 | 現行recall契約の適格性判定を追加 | 有効応答でもTransientOnly＋5reason。未知契約/不正sourceRef拒否、通信・保存0件 | adapter |
| M2-21 | adapter試験 | world/runtime_frame_tests.rs（新規） | R2/R10 | Scopeの負例をtable-drivenで追加 | 同名別Project、複数参照中一件不許可、失効Project、低classificationで内容漏洩0 | adapter |
| M2-22 | meeting統合試験 | meeting/mod.rsのtest補助・world/runtime_meeting_tests.rs（新規） | R3/計画M2-A/B/J | 実meeting_sessionsとowner snapshotでpause/stop/discard/reconcileを検証 | 古いFrameChanged、terminalでactive Focusなし、再起動active復活なし。ASR起動0 | meeting |
| M2-23 | coding統合試験 | world/runtime_coding_tests.rs（新規） | R4/計画M2-C/D/J | 実coding表でstate変化・settled・recoveryの結果を検証 | revision据置のstoppingを検出。settledで成果成功を断定しない。pi process起動0 | coding |
| M2-24 | adapter統合試験 | world/runtime_frame_tests.rs | R5/R6/計画M2-F/G/I | WorldSlice＋Runtimeの取得→Goal撤回→Source忘却→再構築を検証 | graph除外理由が正しく、旧Frame再利用不可。容量超過でも許可Runtimeは返す | adapter |
| M2-25 | snapshot試験 | world/runtime_frame_snapshot_tests.rs（新規） | R6 | 一時file DBをSqliteReaders::openで読み、変更を前後に挟む | 非transaction fixtureで成功を装わない。前後live差・epoch差・policy差を拒否。reader書込み0 | adapter |
| M2-26 | 予算試験 | world/runtime_frame_tests.rs | R7 | 長い日本語、8参照、notice超過、graph残予算の境界を検証 | max_bytes以下、notice16以内、根拠保持、Runtimeとgraphの合計Node30以内 | adapter |
| M2-27 | 性能試験 | world/runtime_frame_perf_tests.rs（新規） | 計画§6 | 五要素100件・ledger2000件・Runtime8参照でwarm-up5/30sample測定 | Runtime p95<=20ms、Frame/再検証p95<=150ms・Frame最大<=500ms。未達は未完了 | perf |
| M2-28 | 境界試験 | world/runtime_frame_boundary_tests.rs（新規） | 計画§4/8 | DB変更なし、SourceRef偽装なし、Broker未接続、既存Continuity回帰を確認 | migration差分0、World assertion増加0、通常会話入力不変、既存G1〜G4通過 | boundary |
| M2-29 | 記録 | spec/evidence/world-model/m2-results.md | 計画§8〜10 | 全G1〜G6、カードとM2-A〜J対応、性能・未接続・M2B条件を記録 | 代替試験を全体成功と言わない。全カードの具体期待値に証拠あり | final |

## 段階ゲート

- M2-01終了: 前段の結果と性能測定方式を把握。未達・未実施を隠して開始しない。
- M2-13終了: owner状態と認可・Source・容量の境界が固定され、純粋型とAdapter試験が通る。
- M2-20終了: 取得・再検証・Evidence能力判定が内部APIとして動く。
- M2-28終了: lifecycle・忘却・snapshot・予算・性能・既存回帰を満たす。
- M2-29終了: M2A完了。M2B/M3と通常会話接続は未実装として引き渡す。

## コマンドと失敗時の扱い

```sh
# coreカード: NNは対象番号
cargo test --locked --manifest-path crates/personal-state-core/Cargo.toml m2_NN_
bun run check:personal-state

# adapterカード
cargo test --locked --manifest-path src-tauri/Cargo.toml --lib m2_NN_
cargo test --locked --manifest-path src-tauri/Cargo.toml --lib memory::personal_state

# ownerカードは対象試験の後、対応するsuiteも実行
cargo test --locked --manifest-path src-tauri/Cargo.toml --lib meeting::
cargo test --locked --manifest-path src-tauri/Cargo.toml --lib coding::

# 性能専用: 通常テストに重いfixtureを混ぜない
cargo test --locked --manifest-path src-tauri/Cargo.toml --lib m2_27_ -- --ignored --nocapture
```

baseline / finalは全体計画G1〜G6、boundaryはG1〜G4、snapshotは対象試験＋G2/G4。M2-01の既存性能試験は名前d41を維持してよいが、測定方式の期待値をm2_01の補助試験でも確認する。対象filter0件は成功扱いしない。

通常のcompile/test失敗は対象カード内で直す。認可・忘却・Scope・予算・期限の失敗はゲート不通過。仕様変更が必要なら再現と推奨変更を記録してそのカードを設計へ戻し、always trueや空の成功結果で後続へ進めない。既存の別機能エラーはbaselineと分け、無関係な修正を始めない。性能未達を測定回数削減で隠さない。

## 渡す指示の例

```text
World Model M2AのM2-09だけを実装してください。
固定契約R3とM2-08までの結果を入力にします。
core/runtime_frame.rsの会議DB/live対応表とdigest計算、純粋テストだけを変更してください。
active+Active、active+Stopping、active+None、terminal+別sessionをassertしてください。
m2_09_とcheck:personal-stateを実行し、m2-progress.mdへ件数と結果を残してください。
SQL・MeetingRuntimeの状態更新・後続カードは実装しないでください。
```
