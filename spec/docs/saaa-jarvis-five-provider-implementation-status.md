# SAAA：5 Providerの実装状況と根拠

作成・改訂日: 2026-09-26  
状態: J1の観測実装とJ0のLARM実機調査を実施中。Provider設定は変更していない。

[コンセプト・受入基準](saaa-jarvis-five-provider-concept.md)と[Runtime契約](saaa-jarvis-five-provider-runtime-contract.md)を支える部分調査の記録。

今回のユーザー方針を、[Personal AI Concept](.archived/saaa-personal-ai-concept.md)の共通Runtime・記憶・状況理解の原則に沿って具体化する。ローカルのHEAD `cf031f8835a232459e560672c5635d887c16ad7d` と、調査時に存在した未コミット変更を含む作業ツリーを確認した。特にQwen即応、接続準備、診断には作業中の変更があるため、以下はリリース済み機能一覧ではない。

「現状」はコードで確認した内容、「目標」「提案」は今後の設計である。後続のJ0調査では保存済みのHarness接続先とprofileを読み、LARM APIで一時Agent Connectionを確保して5 Providerのclaimを確認した。claimed backchannelとornithへの同時生成も1回確認した。詳細は[J0証跡](../evidence/jarvis-five-provider/2026-09-26-j0-feasibility.md)を参照する。保存済み設定の変更、実マイク・スピーカーの動作確認、p95計測は行っていない。モデル名称はcatalogとclaimに表示された値を記録し、外部の性能値や製品仕様は仮定しない。

照合した未コミット差分の主な対象は、`role_routing/frontend.rs`、`runtime/conversation_provider_route.d/01.rs`、`providers/larm_voice/`、`providers/stream/larm_voice.rs`、`providers/service_harness.rs` と診断UIである（Rustのパスは `src-tauri/src/` 基準）。当時の差分そのもののスナップショットは保存していないため、HEADだけでは調査状態を再現できない。今回の文書改訂では全コードを再照合していない。第4章に今回時点の対象差分を保存したが、以前の調査状態を復元した証拠にはしない。実装着手時には実効buildも記録する。

## 1. 現在の実装で確認できたこと

実装の存在、設定の有効化、実Providerとの接続成功、実機での体験成立は別々に確認する。下表の「あり」はコード上の存在を表す。

| 領域 | 現状 | 目標との差・確認事項 |
| --- | --- | --- |
| 5サービス接続 | Harness診断とLARM契約に `llm/backchannel/asr/tts/embedding` がある | 5サービスを列挙・probeできることは、実会話で5種類を適切に利用する証明にはならない |
| 役割設定 | 設定UIに「Qwen 2B 受付 + Ornith 思考」の適用操作があり、frontend→reasonerのrecipeを生成する | 常時応対するQwenと、思考キューを消費するornithへの分離が必要。ユーザーの現在の保存値・ready状態は未確認 |
| Qwenの軽い推論 | `backchannel-qwen35-2b` を指定し、`enable_thinking=false`、出力上限128 tokens、Toolなしで要求する | これはコードのモデル指定。配備済みモデルの同一性・実tok/sは未確認 |
| Qwenの回答分類 | `kind/reply` 形式で greeting、thanks、nod、answer、handoffを扱う。短い回答はornithを呼ばず完了できる | 実機の意味的な分類精度は未測定。handoff時の発話は「少し考えます。」へ固定される |
| Qwenの入力 | 固定の役割指示と今回の発言を送る | 直前の会話、進行中の仕事、訂正対象を含む軽量Contextはこの要求にない。「それでお願い」の理解が課題 |
| Qwenの応答処理 | HTTPは `stream=true` だが、応答bytesを集めてから本文抽出・JSON解析・発話へ進む | tokenが早く届くことと、早く発話が始まることの間に待ちがある |
| 初回の接続準備 | Qwen初回応答は共有Agent Connectionを待たずHTTPへ要求する。ASR=HarnessかつTTS=system-ttsの場合は共有claimを遅延する条件がある | すべてのTTS配置で同じ独立性が保証されるわけではない。共有GPU・claim競合の計測が必要 |
| ornith側の実行 | `llm` 側でContext構成と既存Tool実行経路を使い、Role Routing台帳を通して結果を採用する | ローカルでornith 1.5が実際に選ばれているかは、claim結果と実modelで確認する |
| 思考中の待機発話 | 対象の音声経路では2秒tickの5回ごと、通常10秒間隔で固定の「はい。」を出す処理がある | Qwenが会話状態を理解して応対し続ける構造とは異なる |
| ASR更新の受付 | 調査時の通常入口はASR確定発話を通常turnへ渡す | 本人の途中認識を1.5秒周期、区切り・後着最終認識を即時にQwenへ届ける構成との差分がある。TTS中の常時聞き取りと自声除去も実機受入が必要 |
| 本人登録・話者照合・自声除去 | 初回実装計画の追加照合で本人登録・streaming verifier・speaker gate、およびnative VoiceProcessingのコードを確認した | 登録なし・filter無効はall-speakers。本人限定モードとして成功にしない。話者ゲートの待ちと、実TTS再生中の本人音声・自声分離を実機で検証する。コードの存在は実効経路・精度の保証ではない |
| 追加発話 | 通常の音声入力は通常turnへ入る。実行中はfrontend側の待機キューへ入り、上限4件 | この入力待ちキューは、Qwenが判断後に登録するornith用の思考キューとは別物。Qwenの応対を止めずに後者へ登録する必要がある |
| 音声とテキスト | 既存の経路試験では、音声はfrontend→reasoner、テキストはfrontend Providerをスキップする | 今回は音声体験を先に揃える。テキストにも2B即応を適用するかは次の設計判断 |
| TTS基盤 | 文・句の分割、idle flush、音声の段階的受信・再生、取消、最終回答の重複抑止がある | 下位のstreaming機能だけでは、上位が本文を渡すまでの待ちは解消しない |
| ornithの早期発話 | Role stepのdeltaは `BufferedRoleStepSink` が保持し、採用前には画面・TTSへ出さない。音声応答モードも通常delta読み上げを抑止する | ユーザー向け応答は採用待ちの保持から分け、句読点単位でTTSへ渡す変更が必要 |
| Context / Memory / World | 履歴投影、Context予算、Scope、generation検証、Personal State、World参照、忘却処理がある | memory投影は `SAAA_MEMORY_ENABLED=1` に依存する。実データを使った継続性は今回未確認 |
| embedding利用 | LARMに `embed_query` と診断がある。一方、Tool検索の構築はローカル `MlWorker` のembedding/rerankerを選ぶ | Harness embeddingがTool検索・Memory検索へ一貫して接続された状態ではない。接続試験と製品内利用を分ける |

### 実装上の補足

Qwenへ送る本文は小さいが、共通のturn入力ロードではScope、履歴、条件付きMemory候補も読む。したがって「Qwen経路から重いContext処理が完全に除かれた」とは扱わない。受付・永続化など必要な処理を維持しつつ、Qwenの応答前に何が待ちを作っているかを計測する。

Contextには既存の履歴投影に加えてSegment用の実装もある。`SAAA_CONTEXT_SEGMENTS` の設定と実際に使われる組立経路を確認してから最適化する。設計書にある構造を、すべて有効な現在の構成として扱わない。

Tool検索のrerankerはembeddingとは別の処理である。5種類という構成に合わせるために、既存の検索品質評価なしで削除したり、embeddingだけで置き換えたりしない。

2026-09-26の[初回実装計画](saaa-jarvis-five-provider-implementation-plan-v1.md)作成時に、ASRの版付きPartial/Final、本人登録・継続照合、native captureでのTTS中継続条件、既存Butler仕事台帳を追加照合した。参照ファイルと不足は同計画の第2章に整理した。通常のcaptureはspeechStartedからsuspendへ進むが、nativeかつbarge-in有効ではdetachを省くため、全経路を一律に「TTS中は停止」とは扱わない。これらの確認に合わせて上表を補足したが、実機検証は行っていない。

## 2. 既存文書との関係とコード根拠

[旧音声統合計画](.archived/saaa-role-routing-voice-integration.md)はLFMが会話、Qwenが思考という構成を記述している。今回の役割名はQwen 2Bが即応、ornith 1.5が主思考であり、モデル名を読み替えるだけでは、直列recipe、追加入力、発話の公開条件の違いを解消できない。

同計画にある独立した応対枠、原文保持、revision、発話所有権、実機評価の考え方は参照できる。一方、専用受付IPCや旧LFM経路への復帰を今回の実装方針として採用するわけではない。現在の通常turn・Role Routingへ接続する設計を先に確定する。

[2026-09-22の疎通証跡](../evidence/role-routing/voice-integration-results.md)は当時のLFMとreasoner=`qwen3.8`の結果である。今回のQwen 2B・ornith 1.5構成の速度や受入完了の証拠にはしない。

| 根拠ファイル | 確認対象 |
| --- | --- |
| [settingsRoleRoutingDefaults.ts](../../src/features/settings/settingsRoleRoutingDefaults.ts) / [RoleRoutingSection.tsx](../../src/features/settings/RoleRoutingSection.tsx) | frontend/reasonerの設定、直列recipe、設定操作 |
| [frontend.rs](../../src-tauri/src/role_routing/frontend.rs) | 5種類のkind、短い返答、handoff、待機発話判定 |
| [conversation_provider_route.d/01.rs](../../src-tauri/src/runtime/conversation_provider_route.d/01.rs) | Qwen HTTP要求・全受信後の解析、frontend完結、ornith Context、定期相槌 |
| [larm_voice/mod.rs](../../src-tauri/src/providers/larm_voice/mod.rs) / [stream/larm_voice.rs](../../src-tauri/src/providers/stream/larm_voice.rs) | owner、共有claimの遅延条件、llm leaseと実送信 |
| [useConversationTurn.ts](../../src/features/chat/useConversationTurn.ts) / [useAmbientVoiceSession.ts](../../src/features/voice/useAmbientVoiceSession.ts) | 音声の通常turn投入、実行中のキュー |
| [role_step_sink.rs](../../src-tauri/src/runtime/role_step_sink.rs) / [voice_response.rs](../../src-tauri/src/runtime/voice_response.rs) / [event_hub.rs](../../src-tauri/src/runtime/event_hub.rs) | 未採用draftの保持、最終回答、TTS投入条件 |
| [streaming_tts/runtime.rs](../../src-tauri/src/voice/streaming_tts/runtime.rs) / [chunker.rs](../../src-tauri/src/voice/streaming_tts/chunker.rs) | 文の分割、合成と再生の基盤 |
| [conversation_inputs.rs](../../src-tauri/src/runtime/conversation_inputs.rs) / [conversation_prepare.rs](../../src-tauri/src/runtime/conversation_prepare.rs) | 共通の履歴ロード、Context構成、ScopeとMemory候補 |
| [memory/README.md](../../src-tauri/src/memory/README.md) / [control_plane/source_window.rs](../../src-tauri/src/memory/control_plane/source_window.rs) | Memory・World・忘却の責務、有効化条件 |
| [larm-session/lib.rs](../../crates/larm-session/src/lib.rs) / [service_harness.rs](../../src-tauri/src/providers/service_harness.rs) | 5サービスの接続・診断とembedding要求 |
| [tool_selection/mod.rs](../../src-tauri/src/tool_selection/mod.rs) / [worker.rs](../../src-tauri/src/tool_selection/worker.rs) | 実際のTool検索用embedding/rerankerの構築 |
| [butler_route_tests.rs](../../src-tauri/src/providers/larm_voice/butler_route_tests.rs) | 現在の振る舞いを示す模擬経路試験。今回の文書作成では未実行 |
| [app_state.rs](../../src-tauri/src/app_state.rs) / [tool_selection/invocation.rs](../../src-tauri/src/tool_selection/invocation.rs) | 既存の `RunCancellation` とToolの実行所有者。呼出元の切断だけではTool取消にならない |
| [render_session_inner.rs](../../src-tauri/src/voice/streaming_tts/runtime/render_session_inner.rs) / [http_audio/playback.rs](../../src-tauri/src/voice/http_audio/playback.rs) | 既存の合成順序管理・playerへの音声追加。会話全体の単一ownerと詳細な再生位置は今回の提案として別途整備する |

作成時にはコード・既存文書の照合と凍結チェックを行い、凍結チェックは通過した。文書内のローカルリンクも確認した。仕様書ディレクトリ全体の `spec-html check` は参照切れで失敗しており、全体検査の合格は報告しない。作業中に別の変更として既存文書の `.archived` への移動を検出したため、本書からの参照先は保存時点の配置に合わせた。全体検査で報告された86件の参照エラーは移動の影響を含む可能性があるが、原因を全件照合していないため、すべてを移動のせいとは断定しない。

実機の速度・5サービス同時稼働・会話品質の改善は、本書作成によって達成したものとは扱わない。


## 3. 実装時の検証手順

以下は実装時に実行する手順であり、今回の文書改訂で実行済みという意味ではない。

実装時の検証コマンド候補:

```sh
# 役割経路、受付、読み上げの既存回帰
cargo test --manifest-path src-tauri/Cargo.toml butler_route_tests
bun test tests/voice-final-delivery.test.ts tests/voice-pipeline-response.test.ts tests/larm-voice-owner.test.ts

# LARM契約と検索の変更時
cargo test --manifest-path crates/larm-session/Cargo.toml
cargo test --manifest-path src-tauri/Cargo.toml tool_selection

# ASRを変更する場合に必須
bun test tests/ambient-voice-session.test.tsx tests/voice-capture-races.test.ts tests/voice-asr-packet-sender.test.ts
cargo test --manifest-path src-tauri/Cargo.toml voice::streaming_asr

# 初回応答を変更する場合に必須（desktop smokeはmacOS・snapshot・primary conversationが必要）
bun run quality:check
bun run desktop:smoke

# 最終的な凍結確認と通常ゲート
bun run check
```



## 4. 今回時点の未コミット差分の保存



基準commitの対象ファイルを一時ディレクトリへ取り出し、パッチを適用して、生成された全25ファイルのSHA-256が保存時点の作業ツリーと一致することを確認した。再現時もクリーンな基準commitの隔離checkoutでパッチのハッシュを確認し、`git apply --check` 後に適用する。現行の作業ツリーへ重ねて適用しない。

文書のarchive移動、未追跡・ignore対象、ユーザー設定・DB、起動中build、Provider配備状態は保存対象外である。以前の調査状態やリポジトリ全体・実機環境の完全なスナップショットとは扱わない。


## 5. 後続の実装着手とJ0実機調査

上の作成時記録の後、J1の入力dispatcherを追加し、既存Rust ASR監査チャネルから観測専用で接続した。途中認識・最終認識・commit区切りを処理するが、通常のfinal配送、Provider要求、仕事登録、TTSには介入しない。記録するのは版・確定状態・キュー件数等の匿名化した監査情報で、認識本文は記録しない。observerの入力チャネルは上限付きで、満杯でもASRを待たせない。


J0の実Qwen出力が複数行JSONの制御レコードから本文へ続いたため、J2bの純粋な増分解析器も追加した。SSEのdelta境界に依存せずJSONオブジェクトの終端を見つけ、本文を別イベントとして渡す。分割・引用符内の波括弧・不完全レコード・上限を扱う単体試験4件が通過した。通信adapterと通常入口への接続は未実装である。

LARMはAPIのAgent Connection要求によって遠隔実行環境を確保する。保存済み設定を読み取って一時接続を作成し、claimしたProviderへ実際に生成要求を送った。Qwenとornithの同時応答を1回観測し、Qwenが1要求で複数行JSON制御レコードから本文へ続く形式を10/10件で確認した。接続準備は64〜267秒とばらつき、別の試行では210秒経過後もprobing、さらに約280秒のprobing後にfailedとなった。全試行は接続をreleaseした。詳細な数値、未測定条件、J0判定は[J0証跡](../evidence/jarvis-five-provider/2026-09-26-j0-feasibility.md)に記す。実音声のp95、制御項目と分類の受入、固定評価集合、J3以降は未完了である。
