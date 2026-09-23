# Personal World Model M3A 作業カード

作成日: 2026-09-20。全22枚、M3-00〜21。[全体計画](saaa-personal-world-model-m3-plan.md)と[固定契約](saaa-personal-world-model-m3-execution-contract.md)が正本。

## 実行方法

各カードの前提は直前カードと指定S節の完了。実装1〜3ファイル＋試験1〜2ファイルを標準とする。5実装ファイルを超える場合は先に枝番へ分割する。module登録・機械的importは付随変更可。カードごとのユーザー承認やcommitは必須にしない。

下表のcontextは `src-tauri/src/runtime/context/`。新規ファイル名は予定。試験は `m3_NN_具体条件` と命名する。全試験を既存broker.rsへ詰め込まず、対応world_*_tests.rsへ分ける。

## 作業一覧

| ID | 対象 | 契約 | 実装すること | 合格条件 |
| --- | --- | --- | --- | --- |
| M3-00 | spec/evidence/world-model/m3-progress.md | 計画§3/7 | HEAD、dirty差分、M2報告と現在のコンパイル可否を記録 | 報告成功と今回未実施を分離。並行変更を巻き戻さない |
| M3-01 | context/world_source.rs、mod.rs | S1 | 内部要求・outcome・省略enumを定義 | serde受付なし、Prepared field private、通常callerなし |
| M3-02 | world_source.rs | S1 | 明示Project、EntityId seed、run、参照件数を検査・正規化 | 自由文/ExactName拒否、4seed/8ref可、5/9拒否、空要求省略 |
| M3-03 | context/world_render.rs | S2 | 全Frameのcompact JSON＋固定wrapperを実装 | 五要素、unknown、根拠、期限をparseして元値と照合 |
| M3-04 | world_render.rs・対応試験 | S2 | UTF-8とwrapper込み上限、空Frameを判定 | 8192/8704上限、超過は単位省略、根拠だけ削除0 |
| M3-05 | world_source.rs | S3 | 一Frameを一Candidateへ変換 | May/UntrustedData/Base/utility0、digest/cost一致、専用ID |
| M3-06 | world_source.rs | S4 | 既存WorldFrameServiceからprepareと再検証を接続 | prepare/revalidate同一service、非Currentを投入不可、retry0 |
| M3-07 | world_source試験 | S1/S4 | Scope拒否と状態・Source・期限変化を実DBで検証 | 1999可/2000不可/999不可、失効先情報0、本文ログ0 |
| M3-08 | context/world_shadow.rs | S5 | ShadowInput/Summary、必要な入力型のCloneを追加 | summaryは固定enum/数値だけ。元入力を変更しない |
| M3-09 | world_shadow.rs | S6 | baseline composeを実装 | 同じ入力なら既存Brokerと同結果。baselineエラー時prepare0回 |
| M3-10 | world_shadow.rs | S6 | 候補追加後の二回目composeと全Scope包含検査 | compose最大2、部分Scope一致だけで通過しない |
| M3-11 | world_shadow.rs | S6 | 既存selected維持検査・would_displaceを実装 | 同utility0で既存を押し出したらWorld不採用・baseline維持 |
| M3-12 | world_shadow.rs | S4/S6 | 終了時TTL・固定省略理由・run不一致処理 | 処理中期限超過はexpired、再取得0、別run拒否 |
| M3-13 | context/generation_inputs.rs・試験 | S7 | shadow source_kindの先頭拒否を実装 | selected/omitted双方で記録0/dispatch0、planned維持 |
| M3-14 | context/world_shadow_tests.rs | S8 | 実M2service→render→実Brokerの統合 | graphのみ/Runtimeのみ/両方の採否が合格。fake Candidateだけにしない |
| M3-15 | context/world_shadow_tests.rs | S8 | Goal撤回、Source削除、epoch/link/owner変化を挟む | TTL内でも旧候補不採用、revision据置の変更を検出 |
| M3-16 | context/world_render_tests.rs | S2/S8 | 日本語、引用符、偽delimiter、命令的な名前を入力 | JSON復元一致、system/user追加0、Goal成功への変換0 |
| M3-17 | context/world_shadow_tests.rs | S5/S6 | 予算境界・容量省略・既存yellowを確認 | 根拠部分切断0、current instruction1、既存health保持 |
| M3-18 | context/world_shadow_boundary_tests.rs | S0/S7/S8 | 副作用・run分離・非配線を確認 | DB変更0、manifest作成0、通常Provider候補不変、全体cacheなし |
| M3-19 | context/world_shadow_perf_tests.rs | 計画§7 | 合成100投影/2000ledger以内/8ref、warm5・30sample測定 | 追加処理p95<=20ms、全体p95<=350ms、max<1000ms。実件数と採否記録 |
| M3-20 | 関連試験・size baseline | 計画§7 | 対象suiteと全体ゲートを実行 | 0件成功扱い禁止。追加ファイルのsizeを登録し、閾値を緩めない |
| M3-21 | spec/evidence/world-model/m3-results.md | 計画§8/9 | A〜L対応、測定、未接続、M3B課題を記録 | 全カードに証拠。生成品質や製品接続を完了扱いしない |

## 段階ゲート

- M3-07: 認可済みFrameから、寿命付きの非命令候補まで完成。
- M3-13: 実Broker比較と誤dispatch防止まで完成。
- M3-18: 失効、予算、Scope、本文漏洩、非配線の受入試験が通過。
- M3-21: 全ゲート・性能・記録が揃い、M3Bの設計へ進める。

## カードごとのコマンド

```sh
# NNを対象番号に置き換える。対象0件なら試験名を修正する。
cargo test --locked --manifest-path src-tauri/Cargo.toml --lib m3_NN_
cargo test --locked --manifest-path src-tauri/Cargo.toml --lib runtime::context
# 性能専用試験はignore指定。通常suiteの件数と別記録。
cargo test --locked --manifest-path src-tauri/Cargo.toml --lib m3_19_ -- --ignored --nocapture
```

文書・型だけのカードでまだ実行対象がない場合は「試験未追加」と記録し、0件実行を根拠にしない。M3-00/20/21は全体計画のゲートも実行する。試験が失敗したらそのカード内で原因を特定し、後続機能の実装で隠さない。

## 実装担当への指示例

「M3-11だけを実装してください。S6に従い、baselineで選択された既存候補がproposedから一つでも落ちたらWorldをwould_displaceとして不採用にします。Brokerの全体sort規則やutility値を変更しないでください。同順位May0で衝突する試験と、全候補が収まる試験を追加し、対象試験とruntime::context suiteの結果を記録してください。」
