# Personal World Model M3A 実装計画 — Broker接続のshadow検証

作成日: 2026-09-20。状態: 次段の実装計画。コードは本計画の作成では変更しない。

上位は[コンセプト](saaa-personal-world-model-concept.md)、前段は[M2計画](saaa-personal-world-model-m2-plan.md)。実装担当は本書、[固定契約](saaa-personal-world-model-m3-execution-contract.md)、[作業カード](saaa-personal-world-model-m3-work-cards.md)をセットで使う。

## 1. 次に完成させるもの

M2AのWorldFrameを、既存Context Brokerが扱える一つの非命令候補へ変換する。実際のBrokerを二回使い、Worldなし／ありの採否、予算、既存Contextへの影響、期限内の再検証結果を比較できるshadow評価経路を完成させる。

五要素を新たに推定・保存する段階ではない。因果と相関、Goalと作業完了、条件不明と条件成立、依存不明と利用可能を区別したまま、既存の理解をContextへ渡せるかを検証する。

この段階の利用者は開発・評価担当である。通常の会話回答はまだ変わらない。実際のProviderへ投入するための期限・出力制御を、未実装のまま有効化しない。

## 2. 段階の整理

| 段階 | 今回の扱い | 成果 |
| --- | --- | --- |
| M2A | 前提。完了報告と現行コードあり | 現在状態を添えたFrameの取得・再検証 |
| M2B | 保留。M3Aの前提にはしない | 会話外Sourceの版・削除・権限が揃った後の永続更新 |
| M3A | 本書で実装 | 候補変換、実Brokerでのshadow比較、失効・予算・漏洩の評価 |
| M3B | 次の独立計画 | generation依存、dispatch／出力ゲート、限定Provider投入 |
| M3C | M3B後 | 自然文からの候補抽出と既存commit契約による採用 |
| M4 | M3B/Cの結果から決定 | 品質評価と段階的な利用者向け有効化 |

M2Bの接続先契約が未成立でも、会話Source由来のWorldとRuntime参照を使うM3Aは進められる。M3Aを通常会話への接続完了とは呼ばない。

## 3. 実装調査で分かった注意点

確認基準はHEAD `848c380` と、その上の未コミットM2A実装。M2A以外にも並行作業の変更がある。着手時にはHEADだけでなくdirty状態と対象差分を記録する。

- `WorldFrameService::prepare_frame / revalidate_frame` が存在する。Preparedのrequest/stampはprivateであり、外部から認可を作り直さない。
- Frameは最大1,000 msで失効する。通常の応答生成がこれを超えた場合、M2Aの再検証は状態が不変でもExpiredになる。TTLを延ばすだけでは生成中の依存検査にならない。
- `runtime/context/broker.rs::compose` はCandidateを採用・省略する。現在のwrapperはPERSONAL_STATEであり、Worldにもそのまま適用される。
- BrokerのScope検査はscope_refsのいずれかが許可されれば通る。Worldはその前にM2Aで全対象を認可する必要がある。
- Brokerの重複判定はsource_id/version/digest。source_kindを区別しないため、World専用のsource_id名前空間が必要。
- `generation_inputs::record` は記録直後にdispatchする。Worldをselectedとして記録するだけでは再検証されない。
- `GenerationHandle::complete` は完了時の検証であり、ストリームの文字やTool callが既に外へ出ていない保証にはならない。Provider側にも個別のgeneration wrapperがある。

M2Aの完了記録には全ゲート成功と性能値があるが、今回の再実行 `cargo test --locked --manifest-path src-tauri/Cargo.toml --lib m2_` は `tool_selection/service.rs:980` の `super::invocation` 未解決でコンパイル停止した。M2試験は実行されておらず、失敗件数0を成功と記載しない。並行実装は本計画作成で修正しない。

## 4. 範囲と配置

| 責務 | 予定ファイル |
| --- | --- |
| 信頼済み要求、候補の寿命管理 | `src-tauri/src/runtime/context/world_source.rs`（新規） |
| Frameの非命令JSON表現 | `src-tauri/src/runtime/context/world_render.rs`（新規） |
| 同一入力からのBroker比較 | `src-tauri/src/runtime/context/world_shadow.rs`（新規） |
| 分割した合成統合試験 | `src-tauri/src/runtime/context/world_*_tests.rs`（新規） |
| module登録 | `src-tauri/src/runtime/context/mod.rs` |
| shadow候補の誤dispatch防止 | `src-tauri/src/runtime/context/generation_inputs.rs` |
| 最小のClone導出が必要な場合 | `memory/context_window.rs`、`context/broker.rs`の入力型のみ |

AppState、DB表、SourceRef、World payload、IPC、Provider protocol、Tool registry、通常turnsの配線は変更しない。M2Aサービスは既存readerとmeeting ownerを注入して再利用する。認可済みfixtureから既存service→新source→実Brokerまで通す統合試験を用意し、fake Candidateだけで接続を完成扱いしない。

## 5. 実装単位

22枚のM3-00〜21。原則は実装1〜3ファイル＋試験1〜2ファイル、一枚につき一つの責務。実装AIには一枚と指定契約節、直前の試験結果を渡す。型、優先順位、省略理由、上限、次段への境界は変更しない。

順序は「baseline→要求型→表現→候補→期限→shadow→境界→統合→測定→記録」。通常Providerへ候補を渡す作業カードは含めない。

## 6. 合格基準

| ID | 条件 | 期待値 |
| --- | --- | --- |
| A | 有効な五要素＋Runtime | 一候補、非命令、元Frameの条件・根拠・unknownを保持 |
| B | 明示Projectなし／複数／対象Scope失効 | World候補なし、他Project情報0 |
| C | 同じ要求・固定clock・同じ正本 | 同じ候補本文/digest、同じ採否 |
| D | Frame取得後のGoal撤回、Source削除、owner変更 | shadow投入不可。元のbaselineは変更しない |
| E | now=expires_at、時計巻戻り | expired。自動TTL延長・再試行0 |
| F | Worldありで予算超過 | World候補を丸ごと省略。根拠・条件だけ切らない |
| G | 既存候補と同じutility | Worldは既存候補の採用を奪わない |
| H | 悪意ある改行・引用符・擬似delimiterを含む名前 | JSON文字列として保持。system/userメッセージを増やさない |
| I | shadowのselectedがgenerationへ誤流入 | 記録・dispatch前に拒否、Provider呼出し0 |
| J | 入力の基準Context | 比較後もbyte単位で不変、current instructionは一つ |
| K | stale／容量超過／ownerなし | 理由を区別。省略を「該当知識なし」と断定しない |
| L | 繰返し・キャンセル・並行run | 全体cacheなし、別runへの転用0、正本への書込み0 |

## 7. 検証と測定

```sh
cargo test --locked --manifest-path src-tauri/Cargo.toml --lib m3_
cargo test --locked --manifest-path src-tauri/Cargo.toml --lib runtime::context
cargo test --locked --manifest-path src-tauri/Cargo.toml --lib memory::personal_state::world
bun run check:personal-state
bun run size:check
bunx --bun spec-html check ./spec/docs --warnings-as-errors
bun run check:local
bun run test:rust-packages
```

対象0件は不合格。コンパイル失敗と試験不合格と未実施を区別する。既存不具合はbaselineと比較し、並行作業のコードを無断で巻き戻さない。最後の全体ゲート未通過なら「機能カード通過・全体ゲート未完了」と記録し、M3Bの着手条件を満たしたとしない。

性能はM2Aと同じ端末・debug・warm-up5・30回・nearest-rank p95を使用。M2A取得＋再検証に加わるrender／二回compose／summaryの処理はp95 <= 20 ms、全経路はp95 <= 350 ms、最大 < 1,000 msを開発ゲートとする。固定clockの時間だけで実性能を認定しない。性能fixtureは五要素100投影・全ledger2,000以内・Runtime8参照、World JSON上限8,192 byte。作成できた件数と実際の採用数を別々に記録する。

これらは開発用の基準であり、応答品質・音声遅延・LLMの命令耐性を保証するものではない。

## 8. 次段M3Bへ渡す判断

M3A完了後、Provider投入前に別計画で次を確定する。

1. dispatchまでの鮮度TTLと、生成中の意味・権限の再検証を別の型・APIにする。M2AのExpiredを無視する変更は禁止する。
2. stream本文・音声・保存・Tool実行のどの境界で検証するか、無効化時に何を公開しないかをProviderごとに決める。complete時だけの検査で済ませない。
3. 選択されたWorldにだけ依存receiptを持たせ、正本が変わった応答を採用しない。再起動でreceiptを復元・再利用しない。
4. 途中失効時の再生成回数、費用、ユーザー向け中断表示、外部Tool副作用との関係を定める。
5. trusted要求の製品側供給元と、Project／Runtime／seedの選択UIまたは既存明示文脈との接続を決める。LLMにauthorizedやProjectを決めさせない。

M3Aのshadow比較は回答品質を測らない。M3Bでは同じ質問で有効性・誤推論・不要参照・失効を含む回答評価が別途必要。

## 9. 完了記録

実装時に `spec/evidence/world-model/m3-progress.md` と `m3-results.md` を作成する。各カードの変更関数、試験名・件数、期待値、未実施、性能、A〜L対応を記録する。本文・会議名・Source内容をログへ残さず、合成fixtureの出力例だけを保存する。

完了条件は22カードとA〜L、全体ゲート、性能、非接続の証拠。M2B、M3B、M3Cは未実装として引き渡す。
