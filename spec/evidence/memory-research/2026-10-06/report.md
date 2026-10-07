# SAAAメモリー改善の根拠と実装順序

2026年10月7日追記：本計画に基づくSAAAとContextStillの実装を行った。[実装範囲・運用設定・検証・残る受入項目](./implementation.md)を参照。以下の調査時点の実装状況と検証記録は、当時の根拠として保持する。

並行計画：[Rustのドメインcrate分割](../../../../docs/plans/rust-domain-crate-migration.md)と同時進行する。本書が記憶の挙動・採用条件を定め、分割計画がコード配置・依存方向・テスト移植を定める。共有する所有者と引渡し条件は、分割計画の第15節と本書の「crate移行との作業順序」に従う。

SAAAで目指すのは、過去の会話を多く保存するだけでなく、本人について必要なことを理解し、訂正と時間変化を反映し、根拠を示しながら会話を継続できるメモリーである。現時点の推奨は、既存のSource・Assertion・Transitionの台帳を活かし、実際の会話への接続、記憶候補の採用判定、時間と訂正の処理、小さな公開Snapshotを順に整えること。HindsightのConsolidationは、その上で品質を比較しながら追加する。

Hindsightは、派生記憶の更新、検索、再統合の実装例として有用である。一方、証拠件数は信頼度そのものではなく、文章の書換えや非同期再生成だけではSAAAの訂正・忘却・公開版の失効契約を満たさない。「最良」の優劣は実測前には確定できないため、単純な台帳と検索を比較対象に残し、機能追加ごとに日本語の記憶品質と費用を測る。

Episodeは、2026年10月6日のユーザー承認により、ContextStillのEpisodeCardと既存Distillerを再利用する方針で確定した。既存モデルには意図・判断・結果・未解決事項・原典参照があり、個人の出来事、時間、版と失効、保持責務の不足を補う。SAAAに同等の生成・保存・検索基盤を新設しない。[ContextStill Episodeの調査記録](/Users/y.noguchi/Code/SAAA/spec/evidence/memory-research/2026-10-06/contextstill-episode-review.md)

2026年10月7日のユーザー指定により、ContextStillがSAAAのSQLiteを自動で読み、vibe memory化する処理は既存の前提とする。その取得・同期経路の新設は、本計画の作業と完了条件から外す。SAAAはContextStillが生成したEpisodeを必要時に検索・参照し、現在状態、Snapshot、訂正・忘却、参照の利用可否を扱う。EpisodeをSAAAで事前生成して送る作業は含めない。自動取得の構成は後述の補足メモに残す。

## 対象版と判断の前提

- 調査日：2026年10月6日。
- Hindsight：取得時の`main`、commit `07150298151d83c212bc3d115f3c3284fd86d0dc`。現行実装とIssueの修正反映を照合するために選定した。[対象commit](https://github.com/vectorize-io/hindsight/commit/07150298151d83c212bc3d115f3c3284fd86d0dc)
- SAAA：HEAD `3f70da6285e6586ff0f542a23dbe43029b09c71d`。参照した`crates/personal-state-core/src`、`src-tauri/src/memory`、`src-tauri/src/runtime/conversation_check`、`src-tauri/src/runtime/context`には、調査時のHEADとの差分がなかった。別領域の既存変更には触れていない。
- 概念正本：[SAAAの全体コンセプト](https://chatgpt.com/space/page_9fc5877949748191b556705128f6a2f5)。取得した現行Pageの第5章を判断基準とした。この記録は調査と実装計画であり、概念正本を置き換えない。

正本では、長期記憶の実装、原典への追跡、時間の整合、本人による訂正・忘却の優先、Pendingからバックグラウンド処理を経たSnapshot公開、Checkpointと追加されるTailの分離が決定済みである。既存のRustとSQLite系の基盤を使い、外部OSSを必須依存にしない方針も決定済みである。Profile・Topic・Project・Procedure・Episode・Archive等は役割であり、それぞれ別の正本DBを作る指示ではない。

実装の接続状況、昇格判定の精度、公開周期、検索の組合せ、評価時の費用上限は、コード確認または測定で決める対象である。Pageの過去時点の進捗記述とコードは区別する。調査時点にはStable Context Compilerへの切替経路があるが、これを長期記憶Snapshotの公開完了とは扱わない。計画更新時には別作業によって旧Ornith接続ファイルが作業ツリーからなくなっていたため、該当根拠を調査対象commitへのリンクに固定した。以下の会話経路は調査時のbaselineであり、実装段階0では変更後の経路を再確認する。

## SAAAの現行実装と優先課題

| 領域 | コードで確認した状態 | 実装で解決する課題 |
|---|---|---|
| 台帳 | SourceKeyはID・version・範囲、SourceRefはdigest・sequence・role・access・可用性を持つ。Assertionは根拠、生成入力、論理依存、複数の時刻を分ける | 第二のFact正本を増設せず、必要な記憶型と履歴を既存台帳へ追加する |
| 状態遷移 | Candidate、Active、Disputed、Supersede、Retract、Invalidate等と、再実行・競合・Scope・削除の検証がある | 機構の正しさと、LLMによる対象選択の正しさを分ける |
| 抽出 | 現行ExtractorはObjective・Constraint・Decision・OpenLoop等の継続状態が中心。出力を検証する処理は既にある | 嗜好・習慣・本人の状態を採用条件付きで広げる。Episode生成はContextStillへ集約し、参照利用と失効を接続する。LLMのActive出力だけで昇格を決めない |
| 抽出入力 | 対象Scope内の現行Assertionをまとめて渡し、48,000 bytesを超えると失敗する。各候補は、その入力群の依存を引き継ぐ | 対象・Scopeで検索を絞り、生成単位を小さくする。実際に見せた入力の依存は落とさない |
| 時間 | coreにはobserved_at・effective_at・recorded_at・valid_from・valid_untilがある。一方workerはobserved_atにsource記録時刻、effective_at・valid_fromに処理時刻を使う | 出来事の時刻、有効期間、知った時刻、処理時刻を抽出・保存・検索までつなぐ |
| ジョブ | durable jobs、lease、checkpoint、再起動時の回復、バックログ補充、キャンセルがある | 新しい独立キューを作らず、既存ジョブに段階と適用条件を追加する |
| 通常の会話 | queue_context→context_window→control_planeのSQLはuser_profile_items・working_state_items・continuity_capsule_itemsを読む | personal_assertionsの失効・Pendingを、会話送信と応答受理にどう接続するかを最初に再現する |
| 公開context | PrefixModeのStable経路は接続済み。未設定時の既定値はLegacy。結果受理と保存前に会話・Scopeの再検証がある | 安定した文章を公開するSnapshotと、その根拠を検証するmanifestを実装する |

根拠：[台帳モデル](/Users/y.noguchi/Code/SAAA/crates/personal-state-core/src/model.rs:160)、[状態遷移](/Users/y.noguchi/Code/SAAA/crates/personal-state-core/src/reducer.rs:157)、[抽出方針](/Users/y.noguchi/Code/SAAA/src-tauri/src/memory/personal_state/worker.rs:12)、[抽出と依存](/Users/y.noguchi/Code/SAAA/src-tauri/src/memory/personal_state/worker_run.rs:30)、[永続化schema](/Users/y.noguchi/Code/SAAA/src-tauri/src/memory/personal_state/schema.sql:1)、[会話の読取経路](/Users/y.noguchi/Code/SAAA/src-tauri/src/runtime/conversation_check/queue_context.rs:173)、[実際の読取SQL](/Users/y.noguchi/Code/SAAA/src-tauri/src/memory/control_plane/source_window.rs:474)、[Stable切替](/Users/y.noguchi/Code/SAAA/src-tauri/src/runtime/conversation_check/context_compiler.rs:6)、[Ornith接続](https://github.com/ugnoguchigxp/SAAA/blob/3f70da6285e6586ff0f542a23dbe43029b09c71d/src-tauri/src/runtime/conversation_check/queue_runtime/ornith.rs#L49)。

`personal_state/projection.rs`の`context_candidates`はtestまたはoffline-contracts条件付き、同ファイルの`compose`はtest限定である。そこでの検証を、そのままproductionの会話でPersonal Stateが使われている証明にはできない。[該当条件](/Users/y.noguchi/Code/SAAA/src-tauri/src/memory/personal_state/projection.rs:12)

Profile・Project・Topicの不変本文と公開manifestを持つ新しいSnapshot管理は、調べたmemory・runtime context関連のschemaと呼出経路では確認できなかった。既存のcontinuity capsuleのrevisionやcontextのScope epochは足場になるが、それだけで正本PageのSnapshot公開契約を満たすとは結論しない。

## Hindsightの全体構造

入口は`MemoryEngine.retain_async / retain_batch_async`、検索は`recall_async`、反復検索と回答は`reflect_async`である。主な実装は`hindsight-api-slim/hindsight_api`にある。[Retain入口](https://github.com/vectorize-io/hindsight/blob/07150298151d83c212bc3d115f3c3284fd86d0dc/hindsight-api-slim/hindsight_api/engine/memory_engine.py#L6361)

入力Document・ChunkからLLMでFactを抽出し、Factの本文、時刻、embedding、Entity、検索用リンクを保存する。ExperienceとWorldはFactの区分であり、SAAAのWorld Modelと同一の概念ではない。未統合のFactをバックグラウンドConsolidationで処理し、Observationを作成・更新・削除する。Mental Modelは、指定した問いを軸に記憶から生成する文書である。Knowledge Pagesは、その文書を木構造で整理する機能として対象版に存在する。

これは全入力が必ずRaw→Fact→Observation→Mental Modelと昇格する固定パイプラインではない。Retainには抽出方式の設定差があり、Mental Modelの生成・更新は別の契機を持つ。LLMが抽出・統合・文書生成を担い、DBが保存・Scope・ジョブ制御・原典生存の再検証を担う。Retainの呼び方による同期・非同期と、Consolidationのジョブ処理は分けて考える。

## Consolidationから参考にする仕組み

### 更新と同一性の判定

Consolidationは関連Observationと新しいFactを集め、LLMにcreate・update・deleteを提案させる。updateは旧本文を履歴へ記録したうえで、本文・embedding・根拠ID・時刻を更新する。単なる一回の要約より踏み込んだ更新処理だが、矛盾をどう解消するか、数値や否定を正しく保つかはLLMに依存する。[操作schema](https://github.com/vectorize-io/hindsight/blob/07150298151d83c212bc3d115f3c3284fd86d0dc/hindsight-api-slim/hindsight_api/engine/consolidation/consolidator.py#L703)、[更新適用](https://github.com/vectorize-io/hindsight/blob/07150298151d83c212bc3d115f3c3284fd86d0dc/hindsight-api-slim/hindsight_api/engine/consolidation/consolidator.py#L2752)

候補の類似度だけで統合せず、dedup adjudicatorで同じ主張かを確認する処理がある。また統合候補検索には、通常の質問回答向けrankingと異なるinterleave方式を使う。複数の検索に現れる一般的な候補が、semantic検索の最も近い候補を追い出すのを避けるためである。[同一性判定](https://github.com/vectorize-io/hindsight/blob/07150298151d83c212bc3d115f3c3284fd86d0dc/hindsight-api-slim/hindsight_api/engine/consolidation/consolidator.py#L275)、[統合候補検索](https://github.com/vectorize-io/hindsight/blob/07150298151d83c212bc3d115f3c3284fd86d0dc/hindsight-api-slim/hindsight_api/engine/consolidation/consolidator.py#L2962)

SAAAでは、訂正可能な型とsemantic keyによる照合を先に使う。意味検索はその補助とし、「回答に役立つ情報」と「同じ記憶を更新する相手」を別の検索目的として評価する。

### 証拠件数と独立性

Observationは元FactのIDを持ち、PostgreSQL実装ではdistinctなsource ID配列とproof_countを更新する。これは追跡と重複排除に有用だが、独立した証拠件数や校正されたconfidenceではない。同じ発言の再投入・転載・再抽出は、IDが違っても同じ出所の場合がある。[重複排除](https://github.com/vectorize-io/hindsight/blob/07150298151d83c212bc3d115f3c3284fd86d0dc/hindsight-api-slim/hindsight_api/engine/consolidation/consolidator.py#L695)、[保存処理](https://github.com/vectorize-io/hindsight/blob/07150298151d83c212bc3d115f3c3284fd86d0dc/hindsight-api-slim/hindsight_api/engine/memories/pg/consolidation.py#L227)

調べたConsolidationの操作schemaと保存処理には、独立性を評価した確信度計算や、支持・反証・置換をそれぞれ型付き証拠として保存する契約は確認できなかった。SAAAでは、同じ出所と確認できる原典のoriginをまとめ、独立性が不明なら不明として扱う。支持根拠と生成時に見せた入力は区別する。本人の明示的な嗜好は一件でも採用し得る。推測した習慣は、別の状況での支持と反例を評価して保留・昇格させる。回数だけの一律閾値は設定しない。

### 競合と削除

遅いLLM処理をDB transactionの外で行い、適用前に原典の生存と変更を再検証する構造は参考になる。ジョブ投入時の重複排除とは別に、claim時にもbank単位の直列化がある。[batch適用](https://github.com/vectorize-io/hindsight/blob/07150298151d83c212bc3d115f3c3284fd86d0dc/hindsight-api-slim/hindsight_api/engine/consolidation/consolidator.py#L2263)、[claim契約](https://github.com/vectorize-io/hindsight/blob/07150298151d83c212bc3d115f3c3284fd86d0dc/hindsight-api-slim/hindsight_api/engine/db/ops.py#L154)

原典削除時には影響するObservationを無効化し、生存する根拠を再統合対象へ戻す。Document削除、全置換、deltaでのChunk削除を同じ問題として追う必要がある。deltaの削除経路にも派生Observationを先に掃除する処理がある。[派生記憶の削除](https://github.com/vectorize-io/hindsight/blob/07150298151d83c212bc3d115f3c3284fd86d0dc/hindsight-api-slim/hindsight_api/engine/retain/fact_storage.py#L147)、[delta経路](https://github.com/vectorize-io/hindsight/blob/07150298151d83c212bc3d115f3c3284fd86d0dc/hindsight-api-slim/hindsight_api/engine/retain/chunk_storage.py#L45)

SAAAでも、原典失効を派生記憶と公開contextに伝播させ、残る根拠から再生成する。一部の根拠だけを消して、既に生成された文章が残存根拠だけで成立すると決めつけない。既存の入力依存と削除契約を保持し、不要な広がりは生成前の入力選択を小さくして抑える。

## 時間と過去の情報

Hindsightはmentioned_atとoccurred_start・occurred_end等を分ける。対象版の抽出処理ではmentioned_atの基準に入力のevent_dateを使い、抽出位置に由来する秒単位のoffsetを時刻へ加える。event_dateの設定と個々の発言時刻の扱いは、過去の会話をまとめて取り込む場合に重要である。[抽出時刻](https://github.com/vectorize-io/hindsight/blob/07150298151d83c212bc3d115f3c3284fd86d0dc/hindsight-api-slim/hindsight_api/engine/retain/fact_extraction.py#L3430)、[offset](https://github.com/vectorize-io/hindsight/blob/07150298151d83c212bc3d115f3c3284fd86d0dc/hindsight-api-slim/hindsight_api/engine/retain/fact_extraction.py#L3582)

この構造だけで「2024年の事実を2026年に取り込んでも現在の状態を上書きしない」とは保証できない。原典に正しい時刻を渡すこと、取り込み順と出来事順を分けること、現在の主張との比較が必要になる。per-factの原典時刻を改善する[PR 3717](https://github.com/vectorize-io/hindsight/pull/3717)は対象版の調査時点で未mergeだった。

SAAAでは、原典の記録時刻、出来事の時刻、有効期間、抽出・保存時刻、因果によらない順序sequenceを分ける。年しか分からない出来事に月日を捏造しない。相対日時は原典の発言時刻を基準に解釈し、現在時点の質問と過去時点の質問で利用する値を変える。既存coreの時間列は活かすが、workerの現在時刻による初期化をそのまま履歴Factの有効時刻に使わない。

## Mental Modelと公開context

Mental Modelはsource_queryに対する生成文書で、本文、サイズ上限、更新trigger、生成結果に紐づく根拠、更新時刻と入力watermark等を管理する。更新処理には全体更新と差分を扱う経路がある。`last_refreshed_at`と`last_memory_seen_at`を分ける点は、処理した時刻と読んだ情報の範囲を混同しない設計として参考になる。[更新処理](https://github.com/vectorize-io/hindsight/blob/07150298151d83c212bc3d115f3c3284fd86d0dc/hindsight-api-slim/hindsight_api/engine/memory_engine.py#L17160)

ただし、根拠削除後の文書更新は、自動更新するモデルを対象にしたbest effortの非同期処理である。手動更新のモデルを同じようには更新せず、LLM不在や失敗時には再生成できない。この経路を、SAAAの古い公開版の即時利用停止にそのまま使うことはできない。[削除後のrefresh](https://github.com/vectorize-io/hindsight/blob/07150298151d83c212bc3d115f3c3284fd86d0dc/hindsight-api-slim/hindsight_api/engine/memory_engine.py#L20169)

SAAAでは公開Snapshotを次の三つに分ける。

1. **本文**：不変のbytesとcontent digest。同じ本文を安定した順序で再利用する。
2. **公開manifest**：対象Assertion、原典version、生成入力の依存、有効期間、Scope、policy、公開revision。本文が同じでも根拠や適格性が変われば版を更新する。
3. **現在の利用可否**：失効情報と、関連する未処理入力を送信前・結果受理時に検証する。

manifestの版番号を毎回モデル入力へ埋め込む必要はない。本文の同一性と、公開版の正しさを独立に管理できる。依存する原典の検証だけでは、まだ旧Snapshotの依存に含まれていない新しい訂正を見逃すため、関連Pendingの確認も必要である。

通常の追加情報はバックグラウンドで次の公開版にまとめ、直近の会話はTailへ追加する。訂正・忘却・Scope失効・安全上の制約変更は、公開周期を待たず影響する旧版を止める。既に送信した情報や既に再生した音声を取り消せるとは保証せず、訂正操作の受理後に旧版の生成結果を新たに採用・表示・再生しない境界を実装する。

## RecallとReflectの適用範囲

Recallはsemantic・lexical・graph・temporal等の候補を集め、候補上限、融合、reranking、出力budgetを使う。RRFの基本実装は順位由来の加点であり、複数armに現れる候補を上げる。すべての検索を使えばすべての質問で良くなる、という保証ではない。[Recall入口](https://github.com/vectorize-io/hindsight/blob/07150298151d83c212bc3d115f3c3284fd86d0dc/hindsight-api-slim/hindsight_api/engine/memory_engine.py#L8676)、[融合処理](https://github.com/vectorize-io/hindsight/blob/07150298151d83c212bc3d115f3c3284fd86d0dc/hindsight-api-slim/hindsight_api/engine/search/fusion.py#L29)

Reflectは、Mental Model検索、Observation検索、recall、文書読取等のtoolを使う、回数上限を持つ反復エージェントである。初期のtool選択を制約する経路はあるが、固定された全段階のDB階層を毎回降りる処理ではない。根拠が得られた場合の終了や、上限時の処理もある。[反復と初期tool制約](https://github.com/vectorize-io/hindsight/blob/07150298151d83c212bc3d115f3c3284fd86d0dc/hindsight-api-slim/hindsight_api/engine/reflect/agent.py#L1173)

SAAAの初期構成は、Scope・対象・期間のfilterとFTSを基本にする。言い換えにFTSが届かないケースへembeddingを追加し、時間質問にはtemporal filterを使う。Entityは同定と対象解決に使う。Graphと複数回のReflectは、単純検索が失敗する関係・複数エピソードの問いで効果を確認してから追加する。

同一Topicの会話では選択した参照集合を固定し、必要時にだけEpisodeや原文へ降りる。毎ターンすべてを再検索しない。ただし、Scope・訂正・忘却による失効検証は毎回行う。

## Memory Gateと責務の境界

HindsightのFact抽出promptには、重要・持続的な情報を選択し、挨拶等を落とす方針がある。調べた標準抽出経路では、その選択がLLM抽出に含まれている。SAAAでは保存する会話原典、抽出する候補、長期記憶として採用する主張を別の判断にする。[抽出方針](https://github.com/vectorize-io/hindsight/blob/07150298151d83c212bc3d115f3c3284fd86d0dc/hindsight-api-slim/hindsight_api/engine/retain/fact_extraction.py#L1058)

Gateは短さではなく、発話者、対象、確定度、Scope、時間、再利用の必要性で判定する。「それでいい」は、一意な提案への本人の承認ならDecision候補にできる。指示対象が曖昧なら確認待ちにする。assistantの提案、引用、仮定、第三者の発言を本人の方針へ変換しない。固定設定や権限を、学習した嗜好で上書きしない。

| 機構 | 採用方式と配置 | 最初の実装での扱い |
|---|---|---|
| 原典付き候補と状態遷移 | 既存台帳を拡張しSAAA Memoryへ配置 | 最優先 |
| Observation | 設計を参考に独自実装しSAAA Memoryへ配置 | 型と依存を持つ派生Assertionとして、効果測定後に追加 |
| Mental Model | 小さなProfile・Project・Topic Snapshotとして独自実装 | 確定したAssertionの安定表示から始める |
| 非同期Consolidation | 既存Writerとdurable jobsへ段階を追加 | LLM外の検証・再試行・失効を先に実装 |
| 証拠重複排除 | 設計を参考に共有基盤へ配置 | source IDに加えoriginを追跡 |
| 時間filterと検索 | SAAA Memoryへ配置 | 現在と過去の検索を分ける |
| graph検索用リンク | 必要時にMemoryの検索補助へ配置 | 初期は保留。品質差が出るケースで判断 |
| 因果・条件・依存の理解 | World Modelの既存契約を使う | HindsightのWorld区分やmemory_linksを正本として持ち込まない |
| Knowledge Pages | 外部知識と再利用可能な経験はcontextStillの責務 | SAAAに第二のWikiを作らない |
| Episode生成と保存 | ContextStillのEpisodeCardとDistillerを再利用する方針で確定 | SAAAは原典・即時失効・参照の応答適格性を担当。ContextStillは蒸留・採用・重複整理・保存・再生成・検索を担当する。SQLiteの自動取得とvibe memory化は計画外の既存前提とする |
| Reflect | 共通の予算付きtool実行へ配置 | 通常会話で常時実行せず、証拠不足時に限定 |

HindsightのEntityとmemory_linksは、対象とtemporal・semantic・causalな検索用接続を扱う。PostgreSQLではrelationとして保存され、専用Graph DBが必須ではない。一方、「ユーザーが現在何を好むか」の訂正可能な正本や、SAAAの条件付きWorld Modelの代替と見なすべきではない。[link生成](https://github.com/vectorize-io/hindsight/blob/07150298151d83c212bc3d115f3c3284fd86d0dc/hindsight-api-slim/hindsight_api/engine/retain/link_creation.py)

Knowledge Pagesは対象版の`knowledge_pages`テーブルとMental Modelへの参照により木を作る。生成文書と原典を分ける考え方は参考になるが、個人の古い会話を古いという理由だけでcontextStillへ送らない。共有の可否、削除伝播、用途を確認できる型付きadapterを境界に置く。[Knowledge treeの操作](https://github.com/vectorize-io/hindsight/blob/07150298151d83c212bc3d115f3c3284fd86d0dc/hindsight-api-slim/hindsight_api/engine/memory_engine.py#L19735)

## Issueと修正から得られる教訓

対象は重複・原典削除・時間順序・増大・並行処理の問題群で、openとclosedの本文、コメント、関連PRと現行コードを照合した。以下の6件で、設計判断を変え得る代表例を確認した。

| 事例 | 対象版での判断 | SAAAへの教訓 |
|---|---|---|
| [4799](https://github.com/vectorize-io/hindsight/issues/4799) 証拠IDの大量重複 | 報告では同じID群が膨張しpromptが過大化。[PR 4867](https://github.com/vectorize-io/hindsight/pull/4867)は9月28日merge。対象版にIDの一意化を確認 | 保存・統合・表示の各境界で一意化し、再実行後の件数とbytesを検証する |
| [3294](https://github.com/vectorize-io/hindsight/issues/3294) 原典置換後の孤立Observation | 全置換の掃除に加え、delta削除経路の掃除を確認。[PR 3384](https://github.com/vectorize-io/hindsight/pull/3384)は8月11日merge。3295・3302は未mergeであり、修正の根拠として混同しない | API名ではなく、原典が消えるすべての経路と競合境界を追う |
| [2550](https://github.com/vectorize-io/hindsight/issues/2550) 時間順序 | open。per-fact時刻改善のPR 3717は未merge。対象版のevent_date・位置offsetの制約を確認 | import時刻だけで現在性を決めない。時刻の精度とsequenceを分ける |
| [3249](https://github.com/vectorize-io/hindsight/issues/3249) 複数Retain間の近似重複 | not_plannedでclosed。対象版には追加のdedup機構があるが、意味重複を全面的に解消した実証ではない | 主張単位の分割と同一性を評価する。closedを修正済みと読まない |
| [1284](https://github.com/vectorize-io/hindsight/issues/1284) 原典件数の増大 | not_planned。投稿内でExperienceとObservationの取り違えが訂正され、保管される元Factの増大が論点だった | 派生要約が増えたからRawが圧縮・削除されると思わない。保管期間と検索量を別に管理する |
| [4368](https://github.com/vectorize-io/hindsight/issues/4368) 並行ジョブの重複疑い | 投稿者の再検証で撤回。対象版にはclaim時のbank直列化がある | 投入時のコードだけで競合バグと断定しない。claim・実運用経路・同一原典かを確認する |

4799の一意化は決定的処理として局所検証できた。3249の意味重複、2550の自然言語時刻抽出は、モデルを含めた検証が必要である。Issue報告だけを使って、対象版が今も同じ失敗を起こすと断定しない。

## 推奨する記憶の契約

原典は既存の会話・イベント記録を正本にする。訂正は新しい原典と遷移として記録し、忘却と保管期限に従って利用可能性を管理する。Factは、嗜好・方針・本人の状態など、訂正の効果が大きい型から始める。全情報を万能なsubject・predicate・value形式へ強制しない。単一値、複数値、期間ごとの値を区別する。

Observationは、本文だけの正本にはしない。対象とScope、主張の種類、有効期間、根拠、反証、推論で作られたこと、生成条件を持つ派生Assertionとして扱う。初期は小さな主張単位で生成し、複数主張を含む文章を導入するなら本文の部分とAssertionの対応を残す。本人の明示的な発言が、必ずObservation生成を待ってから確定する構造にはしない。

最初のデータ契約は次を必要とする。これはschema追加の案であり、現行の実装済み列一覧ではない。

| 対象 | 必要な契約 |
|---|---|
| 記憶主張 | 既存Assertion IDとsemantic key、型、対象、値と値の単複、Scope、明示または推論、状態 |
| 時間 | 原典記録時刻、出来事の時刻と精度、有効期間、保存時刻、順序。未知を未知のまま保存 |
| 証拠 | SourceKeyと範囲・digest、origin、支持・反証・置換の役割。生成入力と論理依存は既存の別項目を維持 |
| 採用判定 | Gateの判定、理由、policy版、対象解決の確定度、確認待ちの理由 |
| バックグラウンド処理 | source version・処理段階・policy・extractor版を含む冪等キー、lease、checkpoint、適用時の再検証 |
| 公開 | 不変本文、manifest、公開revision、依存集合、Scopeと期限、関連Pendingの検証 |

訂正対象がAssertion ID等で一意なら、受理と同時に旧主張を利用停止し、新値の採用は検証後に行う。「それは違う」の対象が曖昧なら、意味検索の最上位を勝手に置換しない。会話の参照対象とScopeで絞り、解決まで影響する公開単位を保留する。特定できなければ本人に確認する。

自然言語の訂正をすべて検出し、無関係な記憶を一件も止めないことは、LLM分類だけでは保証できない。保証するのは、訂正操作として受理した後の失効と再利用防止である。既存coreの、未処理の新入力で古いActiveをStaleにする保守的な契約を残し、検証済みの影響範囲がある場合だけ狭める。

## 公開と検索の比較

| 方式 | 解決できること | 残る問題と採用判断 |
|---|---|---|
| 現状維持 | 既存の継続情報と近い会話の参照、個々のcore契約を使える | Personal Stateと通常会話の接続、時間・昇格・公開版の要件を埋める必要がある |
| 台帳と原文FTS | 少ない生成費用で明示情報を追跡し、訂正と削除を検証しやすい | 言い換えや離れたEpisodeの統合に弱い可能性。必ず比較対象にする |
| 台帳と小さなSnapshot | よく使う確定情報とTopicを安定して公開できる | manifest・失効・公開切替の実装が必要。最初の到達点にする |
| 上記にObservationと条件付き深掘り | 離れた会話から傾向を理解し、必要な原典へ降りられる可能性 | 誤一般化とLLM費用が増える。効果を個別に測って追加する |
| Hindsightサービスまたは全面置換 | 抽出・統合・検索の多くを利用できる | RustとSQLiteの既存契約、設定、削除と公開失効の境界を再構築する負担。今回の推奨にはしない |

通常の入力は、固定instructionとtool定義、選択した小さなProfile・Project・TopicのSnapshot、Checkpoint、追加されるTail、当該ターンの検索結果と現在の指示の順序を安定させる。正確なroleと配置は既存Compilerに合わせて決める。検索結果は未信頼の参照として扱い、命令の権限を与えない。

同じ本文を再利用できることとproviderのcache hitは別である。モデル・API・課金仕様が確定し、同一条件で実測するまでは、ヒット率・削減率を数値で主張しない。cacheのために訂正や忘却を遅らせない。

## 共通6ケースの設計照合

以下は設計上の照合であり、実LLMとアプリを通した6ケースの合格報告ではない。

| ケース | 保存と検索の期待動作 | 公開contextと失敗条件 |
|---|---|---|
| 2024年の出来事を2026年に取り込む | 原典の時刻と出来事の年を保持。現在性を取り込み順で決めない。過去質問では当時の状態を引ける | 2026年の現在値として旧情報を昇格させたら失敗。現workerの時刻初期化は追加実装が必要 |
| 本人が以前の情報を訂正する | 旧値は履歴として残せるが、現在の検索では採用しない。対象が曖昧なら保留 | 一意な訂正の受理後に旧Snapshot・生成結果を新たに使えば失敗。coreの遷移だけで自然言語の対象解決精度は証明できない |
| 同じ証拠を重複投入する | 冪等な操作とoriginの照合。複製十件を独立支持十件にしない | 件数の増大で確信や本文長が増えたら失敗。source ID一意化だけでは異なるIDの転載を扱えない |
| 派生記憶の根拠を削除する | 影響する主張を失効し、残存根拠から必要時に再生成。再試行でも復活させない | 再生成前の旧文書が応答へ残れば失敗。本文・manifest・進行中生成を連携して止める |
| 「それでいい」が方針承認になる | 一意な本人承認なら提案と承認の双方へ追跡できるDecision。曖昧なら確認待ち | 短文だから捨てる、assistantの提案だけを本人方針として保存する、のどちらも失敗 |
| 内部更新後も公開本文が同じ | 本文digestは維持し、根拠・適格性が変わるならmanifestを更新。検索結果は必要時だけ追加 | manifestまで凍結して削除を見逃す、訂正後もcache維持のため旧本文を使う、のどちらも失敗 |

失効の対象にはSnapshotだけでなく、Checkpoint、Tail中の派生要約、再利用する検索結果、旧control_planeの公開データ、進行中の生成も含める。訂正では原典を過去の履歴として残せるが、旧値を現在値として再採用させない。忘却では削除対象からこれらのコピーへ伝播する経路も検証する。

Hindsightの長所は、元Factを残した更新・再統合と検索の実装量にある。SAAAへ持ち込む際の主な追加契約は、型付き訂正、独立証拠、精度付き時間、Snapshot失効と実際の応答経路の接続である。

## 実装順序と完了条件

| 段階 | 具体的な作業と主な接続先 | 完了条件 |
|---|---|---|
| 0 会話経路と評価の固定 | queue_context・context_window・control_plane・personal_stateを追い、合成DBで現在値・Pending・失効が応答に届く経路を再現。既存経路と新台帳の役割を明記。Episode連携E0の参照・失効契約とfixtureを固定 | 通常ビルドの経路で、どの正本と検証を使ったかを再現できる。テスト専用の成功をproduction接続と誤認しない |
| 1 型と採用判定 | model・workerの嗜好等の型、時間精度、訂正対象、origin、独立Gateを実装。対象とScopeでbounded extractionを行う | 遅延取り込み、明示と推論、引用、承認、曖昧な訂正、複製を固定suiteで区別できる |
| 2 失効と応答受理 | 既存source・transition・generation検証を使い、関連Pending、訂正・削除・Scope変更を送信前と結果受理に接続。Episode連携E3の失効を隔離環境で接続 | 受理済みの訂正・削除後に旧版が新たに採用されない。再起動・競合・途中失敗でも復活しない。ContextStillの派生記憶とEpisode参照も失効の検証対象に含む |
| 3 小さな公開Snapshot | 確定AssertionからProfile・Project・Topic本文を決定的に生成し、manifestと公開revisionを実装。Compilerへ接続しCheckpointとTailを分ける | 本文の安定と適格性の更新を独立に検証できる。required contextが欠けた場合は明示的に保留・再構成する |
| 4 ObservationとConsolidation | 既存jobsに、関連候補検索・LLM統合・適用前検証・履歴・再生成段階を追加。LLM待機をtransaction外に置く | 必須の安全契約と事前に定めた品質下限を満たし、台帳とSnapshotだけの方式より品質・費用・継続性の少なくとも一つが改善する |
| 5 条件付き深掘りと調整 | Episode連携E4を会話経路へ接続し、ContextStillのEpisodeから原文への段階検索を使う。必要なsemantic・Entity・graph arm、予算付きReflect、公開頻度を調整 | Episodeの原典版・時間・Scope・訂正と削除を検証できる。検索機能を一つずつ外す比較で効果が残る。平均精度だけでなく誤Profile・時間混同・費用も合格する |

段階3までで、確定情報を安定して使い、訂正と忘却に追従する縦の経路を完成させる。段階4以降は、その経路の上でモデルによる理解を改善する。高度な統合を先に完成させてから失効を後付けする順序は取らない。

データ移行はコピーしたDBで検証する。旧projectionとの切替期間は、書込正本を一つに保ち、影響を観察できる比較読取を使う。古い公開データは、原典の適格性を確認できなければそのまま新しいActiveへ移さない。既存の保存設定・providerを維持し、設定の初期化で接続問題を回避しない。

### crate移行との作業順序

メモリーの改善は現行moduleと既存personal-state-coreで進め、Memory crate、root workspace、affectedの導入を前提にしない。画像preview・media抽出・隔離HTTP hostは並行して進められる。Memory・Context・Conversationの抽出は、移す境界の機能変更と決定的な契約を固定してから行う。実LLM品質評価の完了をすべての抽出の前提にはしない。

| 本計画の作業 | 分割計画との接点と順序 |
|---|---|
| 段階0〜1：会話接続・型・採用 | SourceKey・Assertion・追加した記憶型は既存personal-state-coreを再利用する。core、Scope、投影と同じファイルを移す作業は、進行中の型・機能変更を反映してから行う |
| 段階2：失効・応答受理 | 同じwriter transaction内の検証・保存・rollbackを維持する。Memoryの依存処理とdesktopの横断調停を分け、crateごとのcommitや通知だけによる代用をしない |
| 段階3：Snapshot | 永続本文・manifest・公開版・Pending・失効はMemoryの責務。Contextは送信時の予算・構成を担当し、両者の変換はdesktopへ置く。MemoryからContextへの逆依存を作らない |
| 段階4：Consolidation | personal_jobsとreviewの段階・入力依存・checkpointはMemoryに残す。task-queue抽出へ統合しない。移行の成功と品質の改善を分け、実測前の既定オフを維持する |
| Episode連携E0・E2〜E4 | ContextStillの生成・保存正本とSAAAの検索・原典参照・適格性を維持する。Tool配送、応答依存、同focusでの参照継続は実際の利用側も検証する。SQLite自動取得・vibe memory化は双方の計画対象外 |

同じファイル・契約を扱う機能変更と移動は順番に適用する。移行直前に現在の作業ツリーと未追跡ファイルを含めて、入力版、公開API、SQL・serde形式、テスト登録・feature、既存失敗を記録する。抽出中は対象境界への新しい機能変更の適用を待ち、実装とテストを一緒に移す。利用側の回帰を通した後は新しい所有者で機能改善を再開する。別の境界の作業は継続できる。

crate抽出にschema・保存形式・採用条件の変更を混ぜない。必要なメモリーmigrationは別の変更単位として管理し、移行側は最新の確定済み契約を引き継ぐ。旧位置の再公開は互換入口に限定し、実装、writer、schema正本、Episode正本を二重保持しない。

検証は既存verifyと共有ロックを使う。現行Memoryの対象検証はsrc-tauri、独立した意味論はpersonal-state-coreで実行する。抽出後は新crateの検証に会話保存・失効・IPC等の利用側統合を加える。通常実装後のnormal、コミット前のadvance、大規模移行時のfullを区別する。affectedの導入前や影響を確定できない場合は既存の広いgateを使う。

テストの移植先、SQL・IPCを含む検証入力、具体的なgateと並行編集時の扱いは、[分割計画の第15節](../../../../docs/plans/rust-domain-crate-migration.md#15-メモリー計画との同時進行)にまとめる。[実装記録](./implementation.md)の局所合格と全体gateの既存失敗を引渡し時にも区別し、移行によって未測定の品質・費用・実機受入を完了扱いにしない。

### Episode連携の実装と完了条件

以下は同じ実装計画の作業分解である。E0は段階0、E3は段階2の失効契約と併せて進め、E2の採用品質を確認する。E4の通常会話への公開は段階3のSnapshot基盤とE0・E2・E3の合格後に行う。旧E1のSQLite取得・vibe memory化は計画から除外した。過去の実装記録との対応のため、残る作業番号は維持する。訂正・忘却の受理と旧参照の利用停止は背景LLMの完了を待たない。

| 作業 | 担当と主な変更先 | 完了条件 |
|---|---|---|
| E0 原典参照と利用契約 | SAAAのSourceKey・SourceRefと既存のContextStill永続根拠契約を基に、個人Scope、利用許可、版指定fetch、訂正・削除時の失効を定義する。ContextStillのrepo/globalをそのまま個人Scopeに流用しない | 発話者、原典ID・不変版・digest・範囲、順序、記録時刻、出来事時刻と精度、保持条件を両側で照合できる。許可外・失効済みの参照を応答に使わない |
| E2 会話・出来事の蒸留 | ContextStillのepisode_executorのsource・prompt・quality・persistenceとEpisodeCardを拡張する。作業教訓に偏る現行条件を、個人の出来事・判断・未解決事項を扱う条件へ広げる | 既存canonicalの意図・判断・行動・結果を再利用する。開始・終了時刻と精度、Topic・Entity、全根拠IDと版を保持する。成果や教訓のない出来事に架空の結果を補わず、保存対象外・保留を明示する。生成・採用policy版と入力依存が残る |
| E3 訂正・忘却と再生成 | SAAAは受理時に影響する参照・Snapshot・進行中応答を利用停止し、永続的な失効通知を発行する。ContextStillは同期済み原記録、Episode、FTS等の索引、生成job、保持コピーへ反映する | LLM実行前と保存前に原典版・Scope・生存を再検証する。変更・削除中の保存、遅延通知、再試行・再起動で旧Episodeが復活しない。訂正は履歴と現在値を区別し、忘却は対象コピーを除去する。残存根拠からの再生成完了まで旧本文を使わない |
| E4 必要時の検索と応答利用 | ContextStillのnative_episodes・typed recallに版・時間・個人Scope・失効検証を接続する。SAAAの検索AdapterとCompilerはEpisode参照をTopic／Snapshot manifestに結び付ける | 検索から版指定詳細取得・原文参照まで追跡できる。送信前と結果受理時に適格性を検証し、未検証・失効・Scope外の参照を使わない。同Topicでは参照集合を再利用し、必要な問いだけ深掘りする |

E2の保存検証は合成DBで先行できる。Episodeの応答利用の開始条件はE0・E2・E3の合格とし、自動取得の新設や受入を条件にしない。失効の範囲を確定できなければ、影響し得るEpisodeの利用を保留する。ContextStill停止時は通知を永続保存して再送し、未検証の旧Episodeを回答に戻さない。SAAAの原記録・現在状態・直近会話で通常会話を継続し、必要な過去の根拠を確認できなければその制約を回答へ反映する。

受入fixtureには共通6ケースに加え、個人Scope越境、利用許可取消し、蒸留中の削除、失効通知より遅く届く旧生成、ContextStill停止・復帰を含める。許可外参照・失効後の採用・忘却後の復活・二重適用は一件でも失敗とする。時間抽出と出来事の採用品質は、実LLMを使う日本語suiteで別途測り、合成応答の成功だけで受入としない。

既存27件の回帰確認と、新しい参照・利用・失効fixtureは、各リポジトリで定められた検証経路を使う。両側の通常ビルドを接続して検証し、実装時に追加するテストの実行方法と結果を記録する。決定的な検証を先に通し、その後に実LLMの品質比較を行う。失敗した段階では公開を進めず、原因を修正して該当検証を再実行する。

原典の書込正本はSAAA、派生Episodeの書込正本はContextStillとする。SAAAが持つEpisodeのID・版・利用可否・公開参照は、独立したEpisode正本ではない。両側の連携受入を満たせないときは、未完成の機能として留め、SAAA内の別生成系で穴埋めしない。

### 補足メモ：ContextStillの自動取得（計画対象外）

ユーザーが示した既存構成は、SAAAのSQLite原記録 → ContextStillによる自動取得・vibe memory化 → EpisodeDistiller → EpisodeCardである。本計画は、その生成済みEpisodeを利用する構成を前提にする。SQLite取得Adapter、差分同期、cursor管理、vibe memoryへの保存、Distillerへの投入の新設・設定・受入は作業項目と完了条件に含めない。

前回の実装では取得Adapterも追加した。そのコードと検証履歴は[実装記録](./implementation.md)の補足に保持する。今回の範囲修正は計画書の変更であり、既存コードの削除・設定変更は行わない。訂正・忘却の伝播とEpisode参照の利用判定は引き続き本計画の対象とする。

## ベストを判定する評価

日本語の固定suiteでは、同じモデル、入力、利用可能な根拠、予算で、現行経路、台帳とFTS、Snapshot追加、Consolidation追加を比較する。段階0でbaselineを測り、後の方式を比較する前に品質下限と許容費用を定める。候補検索の正解率と、最終回答の正確さは別に測る。正解は人が根拠を確認し、LLM judgeだけに依存しない。

- 機構の合否：Scope外参照、失効後の採用、忘却後の復活、二重適用、原典削除中の生成、再起動後の復活。決定的な受理・拒否契約で検証する。
- モデルの品質：誤Profile率、訂正対象の誤一致と見逃し、保留率、過去と現在の混同、支持しない一般化、適切な回答不能。言い換えと入力順変更でも測る。
- 継続性：Topicが続く場合の参照保持、未完了事項、プロジェクトと本人情報の混同、Tailの追加後の応答。
- 費用と遅延：foreground latency、背景LLM呼出数とtokens、待ち行列の最古Pending、再試行、抽出失敗、公開までの遅延。
- cache：実際の送信bytesの共通prefixと、providerが返すcache実績を別に記録する。品質が同等の比較で評価する。

最小suiteには共通6ケースのほか、複製と独立証拠の入替、期間ごとの嗜好、複数の同名Project、引用内の命令、ASRの誤認後の訂正、生成途中の忘却を含める。音声への接続では、メモリー失効による旧TTSの利用停止と、既存のマイク入力継続・echo処理の契約を両立させる。

[LongMemEval](https://github.com/xiaowu0162/LongMemEval/blob/main/README.md)の情報抽出、複数会話、知識更新、時間、回答不能の区分は、個人記憶の評価軸として参考になる。[LongMemEval V2](https://github.com/xiaowu0162/LongMemEval-V2)はエージェント作業の状態や手順に範囲を広げており、Project・Procedureの追加評価に向く。SAAA固有のScope、忘却、公開版、音声割込みは自前のsuiteで補う。今回これらのbenchmarkを実行した結果はない。

## 実行して確認した範囲

SAAAの`personal-state-core`のcontinuityテストを隔離したtarget directoryで実行し、17件すべてが通った。Scope、遷移、競合・再実行、時間・原典失効、忘却と派生依存、Pending等のcore契約を確認した。これはLLMの抽出精度、アプリの全経路、公開Snapshot、cacheの合格を示す結果ではない。

Hindsightの対象版から、依存のない一意化と融合関数を取り出し、合成入力で3つのassertionを確認した。3,373個のsource ID入力は3個のdistinct IDになった。同じ候補が複数armに現れる例では、RRFは共通候補を先頭にし、interleaveはsemantic armの先頭候補を保持した。これは関数の決定的挙動を確認する小さな例であり、実DB・LLMを使った検索品質や性能の測定ではない。[検証記録](/Users/y.noguchi/Code/SAAA/spec/evidence/memory-research/2026-10-06/verification.json)

外部LLM、課金API、実際のユーザーDBによる検証は行っていない。Astra highには設計案の検討を依頼し、訂正の受理境界、依存の分離、本文とmanifestの版の区別、実装順序を反映した。今回の計画更新では概念正本のEpisode責務・定義・進捗を同期し、5ブロックの読戻しで一致を確認した。プロダクトコード・保存設定・実DBは変更していない。過去の検証JSONは当時の実行記録として維持する。

## 最初の8問への回答

| 問い | 結論 |
|---|---|
| 既存Memoryを置き換える価値 | 現段階では推奨しない。既存台帳の失効・Scope・生成検証を活かした方が、決定済み契約へ到達しやすい |
| 内部サービスとして使う価値 | 多人数向けサーバ運用や別製品の検証では再評価し得るが、SAAAの現在の構成へ必須サービスを追加する根拠は不足。初期計画に含めない |
| 必要部分を独自実装する方がよいか | はい。原典追跡、候補更新、再統合、検索目的の分離を参考にし、RustとSQLiteの既存契約へ実装する |
| 最優先で参考にする設計 | 生成入力を限定した更新、原典生存の適用前検証、原典削除時の派生失効、冪等なjob、入力watermarkと更新時刻の分離 |
| 問題から何を学ぶか | ID一意化、すべての削除経路の追跡、時間と取り込み順の分離、claimまで含めた競合確認、意味重複と独立証拠の区別 |
| 未実装または未決定の有用な仕組み | Hindsightにある参考機構はObservation更新、Mental Model生成、検索融合。公開Snapshotのmanifest、即時失効、通常会話への台帳接続、型付き時間と採用判定はSAAA側で補う契約。推論Observationと検索armの組合せは評価で決める |
| ObservationとMental Modelのメリットとコスト | 過去の会話から必要な理解を小さく公開できる可能性がある。生成費用、誤一般化、削除後の再生成負担が増えるため、根拠・失効・効果比較が成立条件 |
| 更新とprefixをどう両立するか | 内部更新と公開切替を分け、本文bytesを固定し、manifestの適格性を検証する。通常追加はTailまたは次の公開版、訂正・忘却は即時失効にする |

## 調査を深掘りしなかった範囲

Consolidation、原典削除、時刻、Mental Model、会話読取経路は優先課題として処理と保存先を追った。Recall・Reflect・Entity・Knowledge Pages・Gateは責務と追加条件を判断する範囲で確認した。Oracleバックエンドの全経路、全embeddingモデル、全providerのcache仕様、Entity同定品質、運用規模別の性能は比較実測していない。今回の独自実装と失効優先の結論を変えるには、SAAAの実際の性能不足や、より複雑な検索でしか解けない評価ケースが必要になる。

次の作業で優先して解く不確実性は、公開設計の名称ではなく、通常の会話でPersonal Stateの適格性を使う接続、曖昧な訂正の扱い、Snapshot追加が品質と費用に与える実測差である。実装は段階0から始め、段階3の到達点を確認した上でConsolidationを追加する。
