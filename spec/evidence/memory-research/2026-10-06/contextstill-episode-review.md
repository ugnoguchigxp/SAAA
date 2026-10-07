# ContextStill Episodeの実装とSAAAでの再利用判断

ContextStillには、EpisodeCardの保存、原典からの生成、重複排除、品質判定、検索と詳細取得が既にある。状況・意図・判断・行動・結果・教訓・未解決事項という形は、SAAAが必要とするEpisodeと大きく重なる。同等の生成・保存・検索基盤をSAAAへ新設することは、今回の計画では確定しない。まず既存EpisodeCardの再利用を評価し、足りない契約を特定する。

ただし、現行の生成対象は作業経験を中心としており、個人の会話・出来事をそのまますべて扱う実装ではない。時刻、版指定、原典の訂正・削除、本人単位の利用範囲を補わずに、SAAAの長期記憶をすべて任せられるとは結論できない。

## 調査対象と実行範囲

2026年10月6日、ローカルの`/Users/y.noguchi/Code/contextStill`を調査した。HEADは`d03869a43f6f5c09d7359f5f00e319966ae321d3`。調査前後の作業ツリーはcleanで、ContextStillのコード・設定・実DBを変更していない。

TypeScriptのschemaとrepositoryだけでなく、実際のMCPを担当するRustの`native_episodes`、バックグラウンド生成の`episode_executor`、現行SQLite schemaを確認した。READMEとpackage scriptsは、常駐daemon・MCP・queueがRust、UI APIと明示的なoperator CLIがTypeScript/Bunという構成を示す。[常駐処理の所有](/Users/y.noguchi/Code/contextStill/README.md:138)、[実際のMCP振分け](/Users/y.noguchi/Code/contextStill/crates/context-stilld/src/domains/mcp_lifecycle/native_tools.rs:59)

## 保存されるEpisodeの形

EpisodeCardは`episode_cards`に保存され、原典参照は`episode_refs`へ分離する。source_kindとsource_keyの組は一意である。参照には原典の種類・ID、locator、queryHint、metadataがある。statusはactiveまたはdeprecatedで、分類状態、repo/global Scope、projectRef、重要度、confidence、利用件数、staleAt等を持つ。[現行SQLite schema](/Users/y.noguchi/Code/contextStill/src/db/sqlite/core-schema.ts:427)、[公開型](/Users/y.noguchi/Code/contextStill/src/shared/schemas/episode-card.schema.ts:51)

Rustの自動生成は、さらに詳しいcanonical表現をmetadataへ保存する。TypeScriptのEpisodeCardの上位フィールドだけを見ると、意図・判断・未解決事項の保持を見落とす。

| SAAAで必要な内容 | ContextStillの現行表現 | 一致と不足 |
|---|---|---|
| タイトルと概要 | title、situation/context、observations、action、outcome、lesson | 主な構造を再利用できる。Episode全体の単一summaryとは表現が異なる |
| 意図と判断 | metadata.episodeDistillation.canonicalのintent・keyDecisions | 既にある。observationsにも判断を投影する |
| 結果と未解決事項 | outcome、outcomeKind、canonical.openLoops、antiApplicability.openLoops | 既にある |
| 適用条件と避ける条件 | applicability、antiApplicability、usefulFutureTriggers、failedApproach | 作業先例として必要な条件を保持できる |
| Topicと対象 | domains、technologies、changeTypes、tools | Topic階層や人物・一般Entity一覧と同じ型ではない。拡張または対応付けが必要 |
| 原典と範囲 | refs、parentVibeMemoryId、sessionId、sourceStartOffset・sourceEndOffset、sourceEventStart・sourceEventEnd | 原典へ戻る足場がある。ただし原典のimmutable revisionと本文digestを検証する公開契約ではない |
| 出来事の開始と終了 | 原典読取時にはcreated_atを使う。EpisodeのcreatedAt・updatedAtは保存時刻。sourceEventStart・EndはイベントID | Episodeの開始・終了時刻、出来事時刻の精度、有効期間の契約は確認できない。イベントIDを日時と取り違えない |
| 現在の版と失効 | active/deprecated、staleAt | versionを指定してfetchし、その版の原典適格性を再検証する契約が不足 |

canonicalと保存への対応は、[生成用型](/Users/y.noguchi/Code/contextStill/crates/context-stilld/src/domains/queue_lifecycle/episode_executor/types.rs:99)、[canonical JSON](/Users/y.noguchi/Code/contextStill/crates/context-stilld/src/domains/queue_lifecycle/episode_executor/quality.rs:84)、[保存処理](/Users/y.noguchi/Code/contextStill/crates/context-stilld/src/domains/queue_lifecycle/episode_executor/persistence.rs:82)で確認した。metadataには生成された判断等を保持できるが、任意metadataがあることだけで、時間・権限・失効の意味を双方が検証できるとは扱わない。

## 生成と再実行

現行Rustの自動生成処理は、source_kindがvibe_memory以外なら拒否する。原典はvibe_memoriesと関連agent_diff_entriesから再構成したSourceDocumentである。時間間隔、fileの変化、bytes上限等でsegmentを作り、LLMがtask_episode・failure_episode・decision_episodeを生成する。[入力の限定](/Users/y.noguchi/Code/contextStill/crates/context-stilld/src/domains/queue_lifecycle/episode_executor/processing.rs:29)、[原典読取](/Users/y.noguchi/Code/contextStill/crates/context-stilld/src/domains/queue_lifecycle/episode_executor/source.rs:42)、[segment構築](/Users/y.noguchi/Code/contextStill/crates/context-stilld/src/domains/queue_lifecycle/episode_executor/source.rs:194)

生成指示は、将来の作業判断に再利用できるtask-orientedなEpisodeだけを対象にする。parserはcontext・actionTaken・outcome・reusableLesson等を要求する。品質判定は重要度、再利用性、証拠品質、圧縮品質等の閾値を使い、低い候補を保存対象から外す。そのため、例えば「先週家族と旅行した」という出来事を、作業・教訓のないまま記憶する処理と同じではない。[生成指示とparser](/Users/y.noguchi/Code/contextStill/crates/context-stilld/src/domains/queue_lifecycle/episode_executor/distillation.rs:176)、[品質判定](/Users/y.noguchi/Code/contextStill/crates/context-stilld/src/domains/queue_lifecycle/episode_executor/quality.rs:12)

source keyは親ID、byte範囲、生成kind、distillerの版から作る。冪等化に有用だが、原典本文のdigestやimmutable revisionではない。本文が変わっても、この入力の組が同じなら同じkeyになる。[source key](/Users/y.noguchi/Code/contextStill/crates/context-stilld/src/domains/queue_lifecycle/episode_executor/progress.rs:338)

同じsource keyの保存は重複排除され、近似EpisodeにはLLMによる重複審査もある。原典由来の範囲とcanonicalを保存し、進行状況をcheckpointへ記録する。実際のSplit実行ではquery-onlyの読取と既存SQLite Writerを分離し、保存時にprovider leaseの所有を確認する。[保存とlease](/Users/y.noguchi/Code/contextStill/crates/context-stilld/src/domains/queue_lifecycle/episode_executor/store.rs:145)

これはqueueの並行実行を防ぐ境界であり、原典の版・削除・権限をすべて再検証する境界とは区別する。既存の再試行、foreground優先、中断・再開、部分失敗、保存の原子性は再利用する価値がある。

## 検索と詳細取得

MCPには`search_episodes`と`fetch_episode`があり、検索→必要な候補だけ詳細取得という使い方ができる。検索結果は小さな先例表現、詳細取得はcanonical等を含むmetadataとrefsを返す。`recall_experience`もEpisodeCardを参照する。[MCP契約](/Users/y.noguchi/Code/contextStill/crates/context-stilld/src/domains/mcp_lifecycle/native_tools.rs:212)、[検索結果と詳細本文](/Users/y.noguchi/Code/contextStill/crates/context-stilld/src/domains/mcp_lifecycle/native_episodes.rs:287)、[typed recall](/Users/y.noguchi/Code/contextStill/crates/context-stilld/src/domains/mcp_lifecycle/native_memory_recall.rs:104)

現行Rustのsearch_episodesはProjectの適格性、status、facets等で候補を絞り、本文・refsのtext scoreとimportance/confidence等で並べる。DBにFTSやembedding用の列があることを、このMCP経路がvectorとFTSを融合して検索する証拠にはできない。[実際の検索処理](/Users/y.noguchi/Code/contextStill/crates/context-stilld/src/domains/mcp_lifecycle/native_episodes.rs:17)

fetch_episodeの公開引数はidのみで、指定IDの現行rowを返す。版指定、本文digest、source availability、利用期限、permission revisionを含む根拠再検証結果を提供する契約ではない。fetch自体はdeprecatedも取得できるため、取得できたことを現在有効な記憶の証明にしない。[ID取得](/Users/y.noguchi/Code/contextStill/crates/context-stilld/src/domains/mcp_lifecycle/native_episodes.rs:136)

検索のrepo/globalとprojectRefの分離は、本人・家族・秘密・Project等の利用範囲すべての代替ではない。個人会話のEpisodeを同じ保存先で扱うなら、その対象に必要な認可と利用目的の契約を追加する。今回、サービス全体の認証安全性を監査した結果としては扱わない。

## 原典削除と失効の確認

SQLiteのepisode_refsはEpisodeCardへFKを持つが、ref_valueのvibe_memory IDへFKを持たない。確認したVibe Memoryの削除APIは、vibe_memoriesの指定rowを削除するSQLを実行する。この削除経路に、派生Episodeのdeprecated化や削除の処理は確認できなかった。[FK](/Users/y.noguchi/Code/contextStill/src/db/sqlite/core-schema.ts:478)、[削除経路](/Users/y.noguchi/Code/contextStill/api/modules/vibe-memory/vibe-memory.routes.ts:179)

現行EpisodeのDDLを取り出したメモリー内の合成SQLiteで、同じ原典DELETE SQLを実行したところ、原典0件、Episode1件、ref1件となり、Episodeはactiveのままだった。これはこのschemaとSQLの局所挙動の確認であり、実ユーザーDBや起動中APIによる忘却試験ではない。

自動生成されたrefsのbyte範囲は、原典を読んで再構成したSourceDocument内の範囲である。元データの不変版、digest、原文との座標対応を公開adapterで確認する必要がある。sourceEventStart・Endは範囲の先頭・末尾イベントIDであり、全source event IDsの集合を返す契約や日時の契約と同一ではない。

SAAA側には既に[ContextStillの永続根拠契約の依頼書](/Users/y.noguchi/Code/SAAA/spec/docs/contextstill-world-evidence-contract-request.md:1)がある。そこに挙げられた版指定fetch、再検証、削除・権限失効feed、原文範囲とdigest、lineageの照合を、Episode再利用の共通条件として使う。依頼書を、現行ContextStillが全項目を提供済みという意味にはしない。

## 計画への反映

2026年10月6日のユーザー承認により、ContextStillのEpisodeCardと既存Distillerを再利用する方針で確定した。SAAAでは既存原会話と本人状態を正本に保ちながら、Episodeへの参照、Topicとの対応、利用可否、応答contextへの公開を扱う。同等のEpisode生成worker、保存repository、検索サービスをSAAAへ新設せず、以下の不足を連携の実装対象にする。

1. **会話と出来事の扱い**：既に生成されるEpisodeについて、作業教訓以外の出来事を扱う型・採用条件と原典参照を整える。SQLite取得Adapterの新設は含めない。
2. **時間と対象**：出来事の開始・終了、精度、Topic・Entity、有効期間。既存canonicalとmetadataを使える部分を残す。
3. **根拠の版と失効**：原典版・digest、版指定fetch、変更・削除・利用範囲の再検証と通知。生成途中と応答途中の失効を含む。
4. **利用範囲と保持責務**：個人情報の送信可否、原典所有者、忘却の担当、サービス停止時に使える情報。現在のrepo/globalを無条件に本人Scopeへ読み替えない。

内容の形が大きく重なるため、既存モデルを拡張する。応答への利用許可を検証し、ローカルの原典・本人状態と即時失効の責務は維持する。2026年10月7日のユーザー指定により、SQLiteの自動取得とvibe memory化は既存の前提として計画対象外にする。

概念正本の「SAAAが最近のEpisodeを所有する」という記述は、SAAAが原典・参照と応答利用の可否・訂正と忘却を担当し、ContextStillがEpisodeの生成・保存・再生成・検索を担当する分担へ同日更新した。[SAAAの全体コンセプト](https://chatgpt.com/space/page_9fc5877949748191b556705128f6a2f5)。作業順序と受入条件は[実装計画](/Users/y.noguchi/Code/SAAA/spec/evidence/memory-research/2026-10-06/report.md)の「Episode連携の実装と完了条件」にまとめる。

## Episode生成の確定した分担

標準方式は、vibe memoryの取得経路でSAAAのSQLite原記録を読み、ContextStillが背景で蒸留してEpisodeを作る構成とする。SAAAが事前にEpisodeを生成して送る作業は実装計画に含めない。

既存の前提は、SAAAのSQLite原記録 → ContextStillによる自動取得・vibe memory化 → EpisodeDistiller → EpisodeCardである。本計画は生成済みEpisodeの検索・参照、採用品質、訂正・忘却を対象とする。取得・同期経路の新設や受入は、作業項目と完了条件に含めない。

補足の調査メモ：10月6日に確認したRustには、外部agentログをvibe_memoriesへ保存するとEpisodeDistillerをenqueueする経路があり、その時点のsource enumはCodex・Antigravity・Claudeだった。EpisodeDistiller自体はContextStillのSQLite内のvibe_memoriesを読む。この当時のコード観察を、更新後の計画で取得Adapter追加の要件にはしない。[保存とenqueue](/Users/y.noguchi/Code/contextStill/crates/context-stilld/src/domains/agent_log_sync/store.rs:136)、[調査時のsource参照先](/Users/y.noguchi/Code/contextStill/crates/context-stilld/src/domains/agent_log_sync/types.rs:9)、[Episodeの入力読取](/Users/y.noguchi/Code/contextStill/crates/context-stilld/src/domains/queue_lifecycle/episode_executor/source.rs:42)

| 判断軸 | SAAAがEpisodeを作って送る | ContextStillが原記録から作る |
|---|---|---|
| 生成と品質基準 | SAAAとContextStillの両方に生成仕様が必要になる | 既存Distillerと品質判定へ集約できる |
| 原典からの再生成 | 完成Episodeだけを渡すと、要約で落ちた原典を再取得する接続も必要 | 取得した原典に基づいて生成基準を改善し、再蒸留できる |
| 重複と更新 | 送信前と受信後の重複・更新を整合させる必要がある | 一つの生成・保存系で判定できる。ただし原典版と失効の契約は追加が必要 |
| 即時性 | 会話側で生成すると前景の費用と遅延が増える可能性がある | Episodeは背景生成にし、現在の指示・訂正はSAAAで即時処理できる |

SAAAは、確定した原記録、発話者、IDと版、記録順、出来事の時刻、Scope、取得してよい範囲を提供する。Topicや作業の区切り、本人が承認した判断、検証された結果は補助情報として渡せる。ただし、その補助情報を完成Episodeや検証済みの教訓と取り違えない。

ContextStillは、取得した範囲からEpisodeを生成し、採用判定、重複整理、原典への参照、更新・再生成、保存と検索を担当する。取得範囲・同期頻度・cursor等の管理は既存の自動処理側の責務であり、今回の計画では新設しない。

本人の現在状態、明示的な制約、直近会話、訂正・忘却の受理、応答Snapshotの適格性はSAAA側で扱い、背景のEpisode生成を待たない。原典を訂正・削除した場合は、その版に依存するContextStillのEpisodeと進行中生成を失効させ、SAAAも旧Episodeを応答に再投入しない。

今回の受入では、参照の利用許可、発話者と時刻の保持、訂正・削除の伝播、生成中の原典失効、ContextStill停止中の通常会話継続を検証する。差分取得の再開・cursor・取り込みの冪等性は、この計画の完了条件に含めない。先に確認した27件の既存テストは、SAAAのEpisode利用・失効連携全体の合格を示すものではない。

## 検証結果

- Rustのnative_episodes：12件成功。検索、filter、件数上限、refs、詳細取得とerror処理のfixtureを検証した。
- Rustのepisode_executor：15件成功。合成LLM応答を使った保存、冪等化、部分失敗、再試行、中断・再開、入力検証、Writerとの分離を検証した。
- 現行DDLと原典DELETE SQLの隔離確認：原典を削除しても、Episodeとrefが残り、activeのままだった。

実LLMによる会話抽出の精度、起動中APIとのSAAA結合、個人Scope、訂正と忘却の全経路は未検証である。[実行記録](/Users/y.noguchi/Code/SAAA/spec/evidence/memory-research/2026-10-06/contextstill-episode-verification.json)
