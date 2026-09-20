# Personal World Model G1 実装計画 — 保存済み五要素を回答へ接続

作成日: 2026-09-21。状態: 実装計画。M4A完了報告を前提とするが、着手時に証拠を確認する。

上位は[World Modelコンセプト](saaa-personal-world-model-concept.md)。前段は[M3B](saaa-personal-world-model-m3b-plan.md)と[M4A](saaa-personal-world-model-m4a-plan.md)。本書のG1は機能名であり、旧計画の検証ゲート番号とは別。自然文から永続知識を抽出するM3Cとは区別する。

## 1. 今回完成させる体験

明示Projectがある会話で、利用者が `「Speculative Decoding」は今の目標にどう関係しますか？` と質問したとき、保存済みの五要素を探索し、関連経路、仮説・条件、足りない根拠を通常回答の入力へ渡す。

現在の本番 `runtime/context/world/turn.rs` は `graph_request: None` で、Codingの現在状態だけを扱う。内部には五要素の `activate_v2` とWorldFrameへの包含処理がある。今回はこれを再利用し、既存の保存・推論規則を変更しない。

最初は下記の固定質問形式だけを対象にする。自由な言い換えの認識、LLMによる対象選択、会話からの新しい関係の保存は次段。利用者が試せる形式を検証文書に明示し、「任意の自然文に対応」と表現しない。

## 2. 着手条件と対象外

### G0. 前段の確認

計画作成時、この作業ツリーにはM4A計画はあるが、`m4a-results.md` とM4A評価コードは確認できなかった。別revisionで完了している場合は、そのコードと結果を取り込んだ状態から開始する。本書は完了を取り消す判断でも、完了認定でもない。

次の証拠が揃うまで本番配線カードG1-07以降へ進まない。

- 初回・Tool followupの失効で、実HTTP本文からWorldが除かれる。
- AgentSession等の対象外ProviderにWorldが送られない。fallbackでも同じ。
- 実送信本文とgeneration manifestが一致する。
- World除去時も通常の履歴・Personal State・Tool結果が残る。
- M4Aのケースと改修後の対象試験のrevision・結果が確認できる。

不足する改修は前段として扱い、この計画の機能追加と混ぜない。独立したparser・型・fixtureの作成は先行可能。

### 含めないもの

DB/schema/公開IPC/新envの追加、Worldへの書込み、LLM抽出、外部Evidence永続化、ネットワーク検索、ResearchGapからの自動調査、Meeting復活、steward Goalとの自動統合、任意の雑談へのグラフ探索は対象外。MemoryとProviderの既存有効化条件を維持する。

## 3. 固定実装契約

### C1. 質問形式

新規 `runtime/context/world/question.rs` に純粋関数 `parse_graph_question(text: &str) -> QuestionParse` を置く。

```text
GraphQuestion { topic: String, intent: Relevance | Influence | Correlation | Dependency }
QuestionParse = NotRequested | Invalid | Requested(GraphQuestion)
```

外側の空白をtrimし、次の四つに文字列全体が一致した場合だけRequestedとする。末尾は `？` または `?` のいずれか一つ。句点や前置きの自由な補完は行わない。

| 質問 | intent |
| --- | --- |
| 「<topic>」は今の目標にどう関係しますか？ | Relevance |
| 「<topic>」は何に影響しますか？ | Influence |
| 「<topic>」にはどんな相関がありますか？ | Correlation |
| 「<topic>」は何に依存しますか？ | Dependency |

topicはtrim後1〜160 UTF-8 byte。制御文字、改行、内側の「」を拒否する。空topic・超過topicなど、外形が対象形式で内容不正ならInvalid。形式が一致しない一般質問・引用を含む長文はNotRequested。複数対象や複数質問を分割して探索しない。入力全体は2,048 byte超ならNotRequestedとして走査を打ち切る。

NotRequested/Invalidではgraph_request=None。従来のRuntime状態参照はそのまま継続する。parserの結果をユーザー指示やScope認可として扱わない。

### C2. 正本の質問とScope

対象テキストはruntime_runs.input_message_idが指す保存済みuser/transcriptの現在入力から取得する。同conversation、running run、削除されていないことを確認する。履歴のassistant、Tool結果、recall本文はparserへ渡さない。

既存callerが同じ保存済み入力を保持するなら再読取せず使ってよいが、元message IDとrunの対応を確認する。Providerへ渡す現在のユーザー指示をparser用に書き換えない。

Projectは既存explicit_projectと同じくresolved scopeのfocus/parentから一つだけ。0件・複数・失効はgraph取得なし。質問のtopicにProject名が含まれてもScopeを選び直さない。既存の信頼済みAccessRequestを使い、parserにprincipal/policy/authorizedを作らせない。

### C3. 名称解決

Requestedのtopicを `WorldSeed::ExactName(topic)` 一件へ変換する。名前と明示aliasは、既存query_v2の許可済みEntity集合上のnormalize_name/resolve_seedsだけで解決する。別SQLによる全Project検索、FTS、embedding、LLM、部分一致を追加しない。

現行source.rsのinspect_graph_and_refsはExactNameを拒否するため、その制限を今回の本番入口だけで限定解除する。既存shadow入口の拒否は維持する。実装は内部enum `SeedPolicy::EntityIdsOnly | ExplicitQuestionName` をprivateな共通検査helperへ渡す形とし、公開boolやIPC引数にしない。ExplicitQuestionNameはC1で作ったGraphQuestionからだけ呼べる新規本番関数に閉じ込める。

一致0件はunknown_seed、一致複数はambiguous_seed。候補名やScope外対象の存在を返さず、自動で一件選ばない。名前とaliasが別Entityに一致する場合も曖昧とする。EntityId専用の既存内部APIは維持する。

### C4. GraphRequest

RequestedかつC2認可済みの場合だけ、turn.rsでNoneの代わりに次を渡す。

```text
seeds = [ExactName(topic)]
causal_direction = Forward
limits = LimitsV2::m1().capped()
flags = IncludeFlags::default()
explicit_question = true
```

intentにかかわらず五要素を保持する。intentは回答の焦点であり、他の要素・条件・根拠を削る指示ではない。Reverse探索、比較対象、観測値の推定は追加しない。condition_observations/availability_observations/temporary_attentionは従来どおり空。

Frame8,192 byte、Node合計30、Edge60、因果深さ3、関連深さ4、paths10、fetch/scan500、TTL1,000ms、既存投影100件・ledger2,000件のprofileを維持する。容量検査前に別のledger loadや名称解決を行わない。

Coding参照がゼロでも有効なgraph質問ならFrameを取得する。Runtimeとgraphが両方ある場合も一Frame、一Candidate、既存の単位省略とwould_displace規則を使う。

### C5. 結果と空Frame

通常のWorldSliceV2の全fieldを保持する。因果経路と関連経路、Goal、条件三値、相関、依存、ResearchGap、noticeを独自の短文へ変換してから渡さない。

未知・曖昧・投影stale・pending・容量超過は空の成功知識ではない。既存renderの空Frame省略により、この区別が消える点を今回修正する。

新規本番render helperは、Requestedの場合に限り、graph内のunknown_seed/ambiguous_seed、Frame内のworld_projection_stale/world_pending/world_capacity_omittedのいずれかがある空Frameも既存JSON wrapperで返す。既存enumの実際のserialize名を使用する。任意notice・例外文字列を新たに通さない。通常shadow renderのempty_frame判定は変更しない。

対象Scope不許可やDB破損はこの空Frameへ変換しない。既存の省略/エラー境界を維持する。Scope外にだけ存在する同名対象と本当に未知の対象は、どちらも許可Scope内unknown_seedとして同じ表現にする。

未知・曖昧結果もPreparedWorldFrame、同じTTL、再検証、本文除去を通す。新たに認可済み対象が追加されれば、ledger変更で旧unknown結果を再利用しない。

### C6. 回答方針

既存の信頼済み会話system templateへ、Worldデータがある場合の次の読取規則を短い一節として追加する。templateの現行配置をG1-00で確認する。データからsystem文を組み立てない。

- 問われた対象と明示Project/Goalの関連、成立条件、根拠不足を説明する。
- hypothesisを実測済みとしない。correlates_withを因果に変換しない。
- 条件unknown/unmet、依存unknown/unavailableを区別する。
- unknown_seedは「現在参照できるモデルで対象を解決できない」、ambiguous_seedは対象の明確化を求める。名前候補を捏造しない。
- stale/pending/容量超過は今回の参照制約であり、関連の不存在とは断定しない。
- derived pathを直接観測した事実や保存済みの新規edgeと呼ばない。

Scope、許可、Tool権限、Goal採用はこの回答から作らない。引用は返却された根拠参照の範囲に限定し、取得していない資料の内容やURLを作らない。本文が除去されたrequestでは、Worldを参照できたと答えるよう強制しない。

### C7. 送信と失効

M4Aで検証した同じWorldLive→送信前検査→本文/manifest整合経路を使用する。新しいProvider分岐・別receipt・graph用TTLを追加しない。

Goal撤回、Relation/Source削除、条件や投影revision変更、Scope失効を初回とfollowupの同期点で試験する。失効後は古いgraph、ResearchGap、unknown結果を含む候補全体を除去する。部分的に古い名前や根拠だけ残さない。

出力途中の取消しや実Providerの誤断定防止が完成したとは称さない。今回のoffline試験は送信する根拠と方針を検証する。モデル回答の改善は後続live比較で別途測る。

## 4. 配置と作業カード（15枚）

各カードは実装1〜3ファイル＋試験1〜2ファイル。5実装ファイルを超えるなら枝番へ分割。context/worldは `src-tauri/src/runtime/context/world/`。新規ファイルは予定名。

| ID | 契約・対象 | 作業 | 合格条件 |
| --- | --- | --- | --- |
| G1-00 | G0・evidence | 改修/M4A revision、現module、template、ゲート結果を記録 | 本文の失効/対象外/manifestの証拠が揃う。未達は配線保留 |
| G1-01 | C1・question.rs新規 | enumと固定四形式parser | 四形式、?両種、空/長文/複数/Tool風文字列を試験 |
| G1-02 | C2・question_input.rs新規 | 保存済み現在入力とrun対応を読取 | 別run/assistant/消去済みを拒否、書込み0 |
| G1-03 | C3・source.rs | SeedPolicyをprivateに分離、本番明示質問入口を追加 | shadow ExactName拒否維持。未認可の別resolverなし |
| G1-04 | C4・question.rs | GraphQuestion→GraphRequestの純粋変換 | seed1・Forward・flags全true・上限不変 |
| G1-05 | C5・render.rs/source.rs | 明示質問の固定notice付き空Frameを扱う | unknown/ambiguous/stale等を区別、例外文漏洩0 |
| G1-06 | query統合試験新規 | 正本付き五要素fixtureと名称/alias照会 | 一意一致のみ採用。同名別Project漏洩0、読取更新0 |
| G1-07 | C2/4・turn.rs、conversation_turn.rs | 保存入力からgraph質問を本番composeへ渡す | Projectのみ＋質問でgraphあり。雑談は従来Runtimeだけ |
| G1-08 | C6・既存system template | World読取方針を追加 | ユーザー指示は一つ、データをsystem化しない、placeholder回帰通過 |
| G1-09 | C4/5・統合試験 | 五要素とunknown/曖昧/不成立/競合の送信を検証 | HTTP JSONに根拠・条件・noticeが保持される |
| G1-10 | C7・Provider試験 | Goal/Source/Relation変更を送信前に挟む | 初回/followupで古いgraph全文除去、manifest一致 |
| G1-11 | G0/C7・対象外/fallback試験 | M4A回帰ケースへgraph fixture追加 | AgentSession等へ送信0、非World本文保持 |
| G1-12 | C4・性能試験 | 五要素profileで取得/再検証/整形計測 | 既存容量・byte・p95基準を維持、Errを成功計測にしない |
| G1-13 | §5・全体検査 | suiteと全体ゲートを実行 | filter0不可、既存失敗を分離、size閾値未緩和 |
| G1-14 | §6・evidence/検証文書 | 使える四形式と結果・制約を記録 | 「Coding状態のみ」からの差と未実装を説明できる |

G1-01/04/06のfixture準備はG0待ちでも可能。本番配線後にG0不具合が再発したら、後続機能で埋めず前段の回帰として扱う。

## 5. 受入fixtureと検証

必須fixtureは合成のproject:p、concept:tech、metric:decode、metric:voice、goal:natural、actor:user。Conceptの例と同様、tech→decode→voiceの作用、voice→goalのserves_goal、project→goalのhas_goalを保存済み会話Sourceから構築する。相関とdepends_onを別edgeとして加える。実際の技術効果を示すfixtureではない。

| ケース | 期待 |
| --- | --- |
| 明示した技術→現在Goal | relevance経路、因果はmetricまで、仮説と条件を保持 |
| 相関だけ | correlationを保持、因果経路を捏造しない |
| 条件unknown/unmet | 元の三値を保持、無条件の効果へ変換しない |
| 依存unknown/unavailable | 別値として保持、情報欠如を障害にしない |
| 競合/反証 | disputedと根拠・Gapを保持 |
| 許可Scope内にない対象 | unknown_seed、候補名なし |
| 同名/aliasの複数一致 | ambiguous_seed、自動選択なし |
| graphのみ/Coding＋graph | 一候補、参照整合、総byte/Node上限内 |
| 名称に偽命令/引用符 | JSONデータ、system/user追加なし |
| Goal撤回/Source削除/Relation変更 | 送信前に候補除去、HTTP/manifest一致 |
| 雑談/Memory OFF/対象外Provider | graph照会0または送信0、既存経路維持 |
| 新しい対象の追加後に旧unknownを再検証 | 古い結果をCurrentにしない |

試験名は `world_g1_NN_条件`。合成DBは既存commit経路で構築し、実HTTP fixtureを使用する。実モデル・外部検索・実coding processは呼ばない。

```sh
cargo test --locked --manifest-path src-tauri/Cargo.toml --lib world_g1_
cargo test --locked --manifest-path src-tauri/Cargo.toml --lib runtime::context::world
cargo test --locked --manifest-path src-tauri/Cargo.toml --lib memory::personal_state::world
cargo test --locked --manifest-path src-tauri/Cargo.toml --lib providers::chat_completions
cargo test --locked --manifest-path src-tauri/Cargo.toml --lib providers::agent_session
bun run world:eval
bun run check:personal-state
bun run size:check
bun run spec:check
bun run check:local
bun run test:rust-packages
```

world:evalはM4Aの実装済みコマンドを確認して使用する。存在しない場合はM4A証拠不足として記録し、G1で似た別runnerを新設しない。

性能は同一端末debug、warm-up5、30 sample、p95は昇順29番目。既存profileのFrame/再検証p95<=150ms、取得と整形を含むWorld追加処理p95<=350ms、Frame最大<=500msを維持。parser時間を別列で記録する。fake clockのTTL境界試験と実clock性能試験を混ぜない。初期化・fixture構築時間は別記録。

## 6. 完了条件と次段

実装時に `spec/evidence/world-model/g1-progress.md`、`g1-results.md`、`spec/docs/verification/world-graph-answer.md` を作る。各カードの試験名・結果と、四形式の操作例、必要なProject/保存データ条件を記載する。

完了は、明示質問→認可済み名前解決→五要素Slice→Broker→実HTTP本文まで通り、失効・対象外で除去され、全体ゲートが通ること。固定応答のmock試験から「LLM回答品質が改善した」と結論しない。

次は、会話から五要素の候補を抽出して既存commitへ渡す更新経路と、固定質問群による回答品質比較を別計画で進める。本計画では保存データをfixture/既存正規経路で準備するため、自動学習は未完成である。
