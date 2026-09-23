# SAAA

**Situation-Aware Ambient Agent Runtime**

[English](README.md) | 日本語

**ローカルファースト · 開発中 · MIT · 主な検証対象：macOS**

[起動する](#ローカルで起動する) · [アーキテクチャ](#ランタイムの構成) · [ドキュメント](#ドキュメント) · [開発に参加](CONTRIBUTING.md)

SAAAは、ユーザーの仕事と状況を理解し、目標や制約を覚え、任された範囲でツールを使い、行動するタイミングと静かに待つタイミングを判断するPersonal AIのためのデスクトップランタイムです。状況の観測、永続的な記憶、個人のワールドモデル、役割別のモデル実行、バックグラウンドの仕事を、共通のランタイムへ接続しています。

中心にあるのは、依頼と理解の継続性です。依頼を追跡可能な仕事へ変え、その場のやり取りが終わった後も背景と委任範囲を保持する。結果は仕事の台帳へ戻し、ユーザーの状況に応じて届ける。テキストと音声はその窓口となり、背後の状態管理・実行・配送をランタイムが担います。

このリポジトリでは、そのシステムを実装しています。メモリーとContextの供給、WorldFrameの構成、ツール探索と実行、委任仕事の台帳と報告処理、ロールルーティングの実行器がすでに存在します。利用できる範囲は、設定したProvider、実行プロファイル、有効なサービスによって異なります。目指す体験と設計原則は[Personal AI Concept](spec/docs/saaa-personal-ai-concept.md)、現在の接続・改善課題は本書の後半で説明しています。

## 中核となる循環

```text
ユーザー入力・許可された状況の観測・期限・実行結果
                         │
                  イベントと出典を永続化
                         │
        Personal State・World Model・目標と委任を参照
                         │
       Context BrokerでScope・鮮度・必須Contextを確認
                         │
          役割別モデル選択・ツール選択・実行可否の判断
                         │
               応答する／仕事を進める／待つ／尋ねる
                         │
                  結果の検証と仕事の進捗更新
                         │
          状況に応じた報告・feedbackと学習記録への反映
```

観測されたこと、現在の理解、ユーザーが望むこと、実際に実行されたことは、それぞれ別の記録で管理します。モデルの返答や検索結果だけで実行権限は増えません。仕事の状態は実行台帳が管理し、World Modelはその参照用の状態を構成します。

## 実装されている仕組み

| 領域 | リポジトリにある機能 |
| --- | --- |
| 会話と成果物 | ストリーミング応答、マイク入力、読み上げ、会話の永続化、Scope選択、Markdown/Mermaidの成果物、版とデータsnapshotを持つ生成UI |
| 状況認識 | 前面アプリと入力活動の分類、会話・マイク・音声の状態、シーン判定、確信度、状態変化の安定化、発話・配送方針 |
| メモリーと継続性 | ローカル会話検索、出典付きPersonal State、背景での抽出、訂正と失効、ContextStill経由の経験・ルール・スキル検索 |
| World Model | 型付きの実体と関係、観測と仮説、上限付きグラフ探索、仕事・予定・状況・実行容量の現在状態 |
| 共通Context | generationごとのContext構成、必須・任意情報への予算配分、Scope検査、出典の依存管理、Providerへの最終入力検査 |
| ツールとCapability | Web取得、検索可能なツールカタログ、外部MCP、ローカルMCP gateway、生成したL-Lang/Wasm能力の管理 |
| 委任仕事 | 出典に結び付いた提案、目標と権限、有限stepの計画、read/test recipe、実行待ち行列、取消、復旧、結果検証、永続報告outbox |
| ロールルーティング | モデルactorと役割の設定、有限recipeの実行、レビューと修正、上位モデルへの提案、共通予算、永続実行台帳 |
| 適応・学習 | 判断と結果の記録、Scope付きの明示的な訂正、背景での候補学習、評価gate、有効化とrollbackの処理 |

この表は実装領域の一覧です。すべてが初期状態で有効、または全経路の統合受入が完了していることを意味しません。

## メモリーと継続性

SAAAでは、記憶の用途と管理責任を分けています。

| 層 | 保存・検索するもの | 使い方 |
| --- | --- | --- |
| 会話台帳 | 原文、時刻、runへの参照 | `recall_conversation`で全文検索と期間指定を組み合わせ、必要な会話区間を取得 |
| Personal State | 目標、制約、決定、未決事項、未完了事項、現在の参照対象、進捗参照 | 背景抽出で出典付きの状態を作り、次のgenerationへ供給 |
| 型付き知識検索 | 経験、ルール、スキル | `recall_experience`、`recall_rule`、`recall_skill`でローカルのContextStillを参照 |
| Worldの投影 | 実体、関係、注目対象、根拠 | 現在のScopeに関係する状態とつながりを構成 |
| 学習台帳 | 候補、選択、結果、訂正、policyの版 | 明示的な訂正の反映と、実測に基づく選択方針の改善 |

Personal Stateは、候補・有効・競合・解決済み・置換済み・撤回・無効・古い状態を区別します。訂正前の情報と訂正後の情報を、両方とも現在の事実として扱わないためです。派生した状態には出典とScopeを持たせ、削除や失効を依存先へ反映します。

**Context Broker**は、これらの情報と現在の依頼から、generationごとの入力を構成します。必要な制約や実行継続情報の領域を先に確保し、残りを会話履歴や検索結果へ配分して、送信前に依存関係を検査します。検索した過去の文章は履歴データとして扱います。モデルごとのtoken予算や、更新が続く全adapterの網羅は引き続き改善対象です。

Personal Stateの抽出には`SAAA_MEMORY_ENABLED=1`と抽出先の設定が必要です。型付き知識検索には、利用可能なContextStillサービスが別途必要です。ContextStillの知識DBをSAAAが内蔵するわけではなく、接続できないときに自動でクラウドへ切り替える仕組みでもありません。

実装: [`memory/`](src-tauri/src/memory/)、[`personal-state-core`](crates/personal-state-core/)、[`runtime/context/`](src-tauri/src/runtime/context/)。

## 個人のワールドモデル

World Modelは、プロジェクト、概念、指標、目標、関係者と、それらのつながりを扱います。関係には依存、目標への関連、影響、因果、相関などがあります。観測と仮説を分け、根拠と有効性の情報を保持します。

**WorldFrame**は、次の2つを組み合わせた、その時点の参照用データです。

- 保存済みWorldの状態から、Scopeと探索量を制限して取り出したグラフ。
- coding job、委任仕事、予定、状況、実行容量などを、管理元のruntimeから読み出した最新状態。

Frameには容量上限、有効期間、出典参照、変化を検知するstampがあります。対応するProviderへは共通Context経由で供給します。現在状態への回答は、claim検査やhostが構成するカードを通じて、供給した根拠との対応を確認します。

画面では現在のScopeを選べます。仕事の進捗は管理元から読み出し、グラフ側で独立に更新しません。グラフを質問する入口は現在、明示的な定型文が中心です。自然な言い換えや曖昧な指示語の解決は改善計画に含まれています。また、関係が記録されていることと、因果仮説が証明されたことは区別します。

実装: [`world/`](src-tauri/src/memory/personal_state/world/)、[`WorldFrameの供給`](src-tauri/src/runtime/context/world/)。設計: [Personal World Model](spec/docs/saaa-personal-world-model-concept.md)。

## ツールシステムとCapability

SAAAは、能力を探す処理、契約を読む処理、実行する処理を分けています。カタログの入口は3つの共通ツールです。

1. `tools_search`: やりたいことに合う候補を探す。
2. `tools_describe`: 候補の入出力契約、使い方、結果の続きを取得する。
3. `tools_invoke`: 検証された実行参照を使って呼び出す。

カタログには生成Capabilityと設定済み外部MCPツールを登録します。ローカルembeddingとrerankerによる探索、Scopeに応じた訂正ルール、呼出し台帳を備えています。実行参照、引数、アクセス権、取消、結果サイズはhost側で検査します。探索モデルが不足している場合は機能不足として扱い、勝手に外部モデルへ置き換えません。

外部MCPにはStreamable HTTPで接続します。SAAA自身も、同じ3つの入口を認証付き・loopback限定のMCP serverとして公開でき、アプリと同じカタログ・台帳を使います。このほか会話runtimeには、Web取得、記憶検索、仕事の提案、生成UIなどの直接呼出しツールがあります。

### L-Lang / Wasmによる能力の生成

生成Capabilityには、生成、buildとpackage化、検証、inspection、公開、実行、廃止の処理があります。能力の版を保持し、Wasmは契約と資源上限を検査する別host processで動かします。生成と実行にはProvider、build tool、runtime bundleの設定が必要です。能力の説明文が生成できただけでは、実行可能な状態にはなりません。

詳細は[Capability / Tool Runtime Concept](spec/docs/saaa-capability-tool-runtime-concept.md)、[ツール選択の実装ガイド](spec/docs/saaa-tool-selection-d0-d3-implementation-guide.md)、[L-Lang能力の実装ガイド](spec/docs/saaa-llang-dynamic-capability-implementation-guide.md)を参照してください。

## ロールルーティング

ロールルーティングでは、モデルの実行先を役割へ割り当てます。**actor**はtransport、Provider/model、実行場所、resource group、能力、入力上限を持ちます。**role**はactorを参照し、**recipe**はある判断に必要な役割の実行順を定義します。

| Role | 担当 |
| --- | --- |
| `frontend` | その場の応対と確認 |
| `reasoner` | 主な推論と回答 |
| `advanced` | より難しい再検討 |
| `reviewer` | 別の設定済み実行先によるレビュー |
| `premium` | 設定された承認方針に従う上位モデルへの切替 |
| `tool_specialist` | 共通のツール実行境界を使った専門処理 |

応答、説明、確認、再検討、レビュー後の修正、上位モデルへの提案などを扱います。recipeは依存関係のある有限stepへ変換します。hostはactorの適格性、Context、版、期限、step・tool・reviewの回数、設定された費用上限を確認してから実行します。rootとstepの台帳で取消、古い結果の処理、復旧を管理し、短い相づちや進捗の読み上げは推論結果と分けて調整します。

ロールルーティングはSettingsで有効にして使います。actorを割り当てるだけで任意のProviderが同じように動くわけではなく、transportと能力がその役割に対応している必要があります。選択と学習の記録は品質・遅延・費用の比較に使い、学習済み方針にも適格性と有効化のgateを適用します。

実装: [`role_routing/`](src-tauri/src/role_routing/)。設計: [実行契約](spec/docs/saaa-role-routing-execution-contract.md)、[学習契約](spec/docs/saaa-role-routing-learning-contract.md)。

## 状況認識と委任仕事

状況認識では、会話、会議、coding、執筆、media、集中、solo、不明などを分類します。macOSでは前面アプリと入力からの経過時間をカテゴリへ変換し、入力文字列や画面内容は収集しません。確信度、状態遷移を安定させる処理、信号の取得状態を使い、一瞬の変化や観測できない状態を確定情報として扱うことを避けます。

委任仕事では、**目標・権限・計画・実行記録**を分けます。提案はユーザーの原文と要求された操作へ結び付け、受け付けた仕事を有限の計画、待ち行列、対応するcoding実行プロファイルへ渡します。永続driverが結果を取り込み、条件を満たす次のstepへ進め、報告をoutboxへ保存します。scheduleと状況の方針によって、実行・保留・延期・確認を判断します。

報告処理は、新しいユーザー発話がなくても動きます。定期復旧とwakeの仕組みがあり、完了や保留解除の全経路から速やかに起床させる接続を改善中です。結果の証拠、否定依頼の扱い、統合受入も進めています。現在の実行範囲は対応recipe・profileと委任権限で定まり、任意のデスクトップアプリを無制限に自動操作するものではありません。

実装: [`situation/`](src-tauri/src/situation/)、[`steward/`](src-tauri/src/steward/)、[`schedule/`](src-tauri/src/schedule/)。

## 適応・学習と現在の改善課題

Provider/recipe、tool、plan、notificationの選択と結果を記録します。ユーザーの明示的な指定は学習した順位より優先します。背景workerがdatasetと学習候補を作り、評価、承認、有効化、失効、rollbackを分けて扱います。

実測評価と昇格を通常の管理導線へ接続する作業が残っています。候補を学習できること、効果が実証されていること、実際に有効化されていることは、それぞれ別の状態です。

現在の主な接続課題は[改善5点の実装計画](spec/docs/saaa-review-five-improvements-terra-plan.md)にまとめています。

- 仕事の完了を、その実行に対応する実際の成果証拠で判定する。
- 原文全体と最新の権限から、禁止・撤回された依頼を保護する。
- 学習候補を通常の評価・有効化導線へ接続する。
- 自然なグラフ質問と指示対象をScope内で解決する。
- 完了や状況変化から、報告処理を速やかに起床させる。

## ランタイムの構成

Reactが会話・操作・状態確認の画面を担い、Tauri内のRust runtimeが実行方針、Providerとtoolの境界、永続化を管理します。SQLiteをローカルの正本とし、投影やProvider向けContextは、その記録とruntimeの現在状態から作ります。

```text
src/                         Reactの会話・設定・仕事・成果物UI
src-tauri/src/runtime/       会話の実行制御と共通Context Broker
src-tauri/src/memory/        Recall、Personal State、World Model、背景抽出
src-tauri/src/role_routing/  Actor選択、recipe実行、予算、学習
src-tauri/src/tool_selection/ ツールカタログ、探索、MCP、呼出し台帳
src-tauri/src/generated_capabilities/ L-Lang/Wasm能力の管理とhost
src-tauri/src/steward/       委任目標、計画、実行、報告
src-tauri/src/situation/     状況の観測・分類・注意状態の方針
src-tauri/src/schedule/      永続化された期限と実行判断
src-tauri/src/persistence/   SQLite writer、reader、schema、backup
crates/                     共通Rust契約と中核ロジック
services/                   連携service
contexts/                   s11tnextのsystem context原本
scripts/                    開発・評価・受入runner
spec/                       Concept、契約、計画、証跡
```

## 必要なもの

- [Bun](https://bun.sh/) 1.4.2（`package.json`で固定）
- Rust 1.92.0（`rust-toolchain.toml`で指定。rustfmt・Clippyを含む）
- 対象 OS 用の Tauri 2 ビルド環境（macOSでは`xcode-select --install`でXcode Command Line Toolsを導入）
- ローカル会話経路を使う場合は、プライベートネットワークから接続できるローカル LLM サーバー。`LARM_API_TOKEN`は、サーバーがLAN内の匿名接続を許可する場合は任意、Bearer認証を要求する場合は必須です。
- 音声入力を使う場合は、設定するASRサービスへの接続

主な検証対象は macOS です。OS の音声合成は macOS、Linux、Windows に実装があります。

## ローカルで起動する

ソースから起動します。モデルや音声サービスの接続は、アプリを開いてから設定できます。

```sh
git clone https://github.com/ugnoguchigxp/SAAA.git
cd SAAA
bun install --frozen-lockfile
bun start
```

1. Settingsでモデル接続を追加し、接続を確認して会話用の経路に選びます。OpenAI互換APIのキーは設定画面から登録できます。
2. Chatから短いテキストを送り、応答を確認します。
3. 音声を使う場合だけ、ASRの接続、入力デバイス、マイク権限を設定します。話者フィルターも任意です。

ローカルLLMの接続APIがBearer認証を要求する場合は、起動前に同じシェルで`LARM_API_TOKEN`を設定してください。サーバーがLAN内の匿名接続を明示的に許可する場合だけ未設定にできます。`bun run dev`はフロントエンドの開発サーバーです。デスクトップIPCや音声の確認には`bun start`を使います。

## モデル接続を設定する

### ローカル LLM サーバー

SAAA は設定したホストの接続 API を通じて、会話に使うローカルモデルへの接続を取得します。応答で得た OpenAI 互換の接続先、モデル名、短時間だけ有効な認証情報はメモリ上で使い、各ターンの終了時に接続を解放します。これらの値は SQLite に保存しません。

`LARM_API_TOKEN`を設定すると、SAAAはBearer認証情報として送信します。信頼できるLAN内でサーバーが匿名接続を明示的に許可する場合は未設定にできますが、すべてのサーバーが認証不要という意味ではありません。SAAA は SSH トンネルを作成しないため、接続 API と、サーバーが返すモデル接続先の両方へプライベートネットワークから到達できる必要があります。

音声会話は、Settings → Model Providers で設定した LAN host を共用します。SAAA はプライベート ASR 接続先を導出し、`/v1/models` と `/health` からモデル情報を取得して Settings → Voice へ反映します。ASR専用の環境変数は不要です。

### OpenAI 互換 API

Settings から接続先とモデル名を追加できます。API-key 認証を使う場合は、Settings で Provider のキーを保存してください。キーは macOS Keychain の service `com.saaa.provider-api-key` に Provider ID ごとに保存され、Settings JSON や SQLite には保存されません。この認証情報の保存は macOS 専用です。

現行の OpenAI 互換 Provider は、`SAAA_PROVIDER_<PROVIDER_ID>_API_KEY` や `OPENAI_API_KEY` への fallback を行いません。下記の LARM token は別のランタイム経路で使用します。

### Agent-sessionによる実行

Runtimeにはagent-sessionの連携とcoding実行adapterもあります。routing actorや委任recipeへ割り当てる前に、対応するProviderまたはcoding profileと、必要なローカルruntimeを設定してください。OpenAI互換endpointへの接続だけで同じ実行能力が使えるわけではありません。基本の接続確認とは別に、roleとtoolの適格性を検査します。

### LARM Provider

LARM Provider は既定で無効です。オフライン検証済みの経路を開発環境で試す場合だけ、起動時に次の値を設定します。

```sh
export SAAA_LARM_ENABLED=1
export LARM_API_TOKEN="<token>"
bun start
```

機能フラグは起動時に一度だけ読み込まれます。無効へ戻す場合もアプリを再起動してください。本番トラフィックを有効にする前に、[LARM Operations Runbook](spec/docs/mvp-2.6-larm-operations-runbook.html) と現在の release evidence を確認する必要があります。

### WebFetch ツール

会話モデルには、`llm-fetch` を使った `web_search` と `fetch_content` を提供します。DuckDuckGo 検索は API key なしで利用できます。SAAA の起動時に `BRAVE_SEARCH_API_KEY` が設定されている場合は、DuckDuckGo で再試行可能な失敗が起きたときだけ Brave Search をフォールバックとして使います。

同梱ランタイムは、パッケージのモデル向け toolset と strict Context Guard を使います。検索結果と取得本文は、信頼できない tool data として扱われます。取得できるのは標準ポート上の公開 HTTP(S) URL だけで、任意機能の Playwright レンダリングは含みません。

## 音声プロファイル

Settings → Voice → My voice profile では、利用者本人の声を端末内で照合するフィルターを設定できます。有効化には、10〜12 秒の有効なサンプルを 5 件、合計 50 秒以上登録する必要があります。発音やイントネーションの違いを含む 5 種類の長文が順番に表示されます。文章は意図的に録音時間より長く、全文を読み切らず、約 12 秒で自動停止するまで連続して読み上げます。

音声サンプルは WAV としてアプリのデータディレクトリに、話者埋め込みは SQLite に、いずれも暗号化せず保存します。macOS では保存ディレクトリを `0700`、音声ファイルを `0600` に制限します。フィルターが有効な間は、ローカル照合に通った音声だけをローカル ASR サーバーへ送ります。モデル、保存データ、タイムアウト、話者判定で問題が起きた場合、フィルターを迂回して音声を送ることはありません。

この機能は文字起こし時のプライバシーフィルターです。本人確認や、録音した声によるなりすましを防ぐ認証機能ではありません。

## 追加機能の有効化

基本のテキスト会話はSettingsで設定できます。追加機能にはそれぞれ前提があり、フラグを有効にしても必要なserviceやmodelが自動で導入されるわけではありません。

| 機能 | 設定の入口 |
| --- | --- |
| Personal Stateの抽出 | `SAAA_MEMORY_ENABLED=1`と利用可能な抽出Provider |
| 型付きメモリー検索 | `SAAA_MEMORY_ENABLED=1`とローカルContextStillのendpoint discovery。探索先は`SAAA_CONTEXT_STILL_RUN_DIR`で変更可能 |
| ツール探索・外部MCP | `SAAA_TOOL_SELECTION_CONFIG`でツール選択のJSON設定を指定 |
| SAAAのローカルMCP gateway | `SAAA_TOOL_GATEWAY_MCP_CONFIG`でlistener・認証設定を指定 |
| L-Lang生成 | `SAAA_LLANG_GENERATION_CONFIG`で生成設定を指定。実行にはruntime bundleも必要 |
| ロールルーティング・適応設定 | Role Routing設定でactor、role、recipe、上限、学習条件を設定 |
| 状況認識・schedule | 各設定とOSの権限によって、観測・実行できる範囲が決まる |

設定形式の詳細は各機能のガイドを参照してください。mock/fixtureは隔離した開発用で、実Providerへの対応を証明するものではありません。

## ローカルデータとプライバシー

DBは`com.saaa.desktop`のアプリデータディレクトリへ保存します。macOSでは次の場所です。

```text
~/Library/Application Support/com.saaa.desktop/saaa.sqlite3
```

DBの書込みは一つのRust `SqliteWriter`が管理します。OSのlockで、別processが同じデータディレクトリを書込み用に開くことを防ぎます。読取り専用接続は一貫したtransactionで参照します。稼働中の`saaa.sqlite3.writer.lock`は削除しないでください。終了時にOSがlockを解放します。

会話と設定、Personal StateとWorldの状態、仕事とtoolの台帳、routingと学習の記録、生成viewの状態、構造化監査イベントをローカルに保存します。保存先がローカルでも、推論やtool処理のすべてが端末内で完結するわけではありません。選んだProviderとtoolには、その処理に必要な入力が送信されます。ローカル会話経路を選択した場合、cloudへの自動fallbackは既定で無効です。

Settingsで登録したProvider APIキーはmacOS Keychainへ保存し、設定JSONやSQLiteには入れません。メインDBとvoice profileはSAAAでは暗号化していません。任意の話者filterはWAVをアプリデータディレクトリ、embeddingをSQLiteへ保存します。適用範囲と制約は上のvoice profileの説明を参照してください。

構造化監査ログには、prompt・transcript・モデル出力の原文ではなく、実行の状態や結果などのmetadataを記録します。会話原文は会話の記録へ保存します。Settingsでは整合性のあるSQLite backupと秘匿情報を除いた診断情報を出力できます。DB backupには話者embeddingが含まれますが、WAVや外部の能力・model fileをすべて含むわけではありません。DBだけではvoice profileやruntime全体を復元できません。旧schemaの移行前にはDB backupを作ります。

## 開発と検証

固定された依存関係を導入し、変更に応じた検証を実行します。

```sh
bun run check:local
bun run test:rust-packages
bun run spec:check
```

`check:local`はformat、lint、生成物、型、module size、Rust/frontendの検証を実行します。個別修正では、先に`bun test tests/<file>`または`cargo test --manifest-path src-tauri/Cargo.toml <filter>`で対象を確認します。filter付きの成功は、目的のテストが実際に実行されたことも確認してください。

`contexts/`の変更後は`bun run s11tnext:build`、公開するRust IPC型の変更後は`bun run ipc:generate`を実行し、生成差分を確認します。`bun run build`はfrontend buildとIPC契約検査、`bun run tauri build`はdesktop bundleを作成します。任意のcoverage reportは`bun run test:coverage`で生成できます。

desktopと実serviceを使う受入には、各手順書で指定された環境が必要です。

```sh
bun run desktop:smoke
bun run desktop:e2e --report-dir /absolute/path/to/new-report-directory
bun run readiness:verify --report-dir /absolute/path/to/new-report-directory
```

隔離データと新しいreport directoryを使ってください。smokeの成功は起動とIPC準備の証明です。委任仕事の完遂や学習による改善の証明には、それぞれの検証が必要です。失敗と実行不能は分けて記録し、実接続の未検証をfixture成功で置き換えません。OS・経路ごとの条件は[製品準備状況](spec/docs/product-readiness-status.html)と[受入手順](spec/docs/product-readiness-acceptance-runbook.html)を参照してください。機能フラグ付きLARMの本番経路には、別途[運用条件](spec/docs/mvp-2.6-larm-operations-runbook.html)があります。

## ドキュメント

- [Personal AI Concept](spec/docs/saaa-personal-ai-concept.md): 現在の製品Visionと共通runtimeの原則
- [Capability / Tool Runtime](spec/docs/saaa-capability-tool-runtime-concept.md): 能力の探索と実行の責任分担
- [Personal State architecture](spec/docs/personal-state-architecture-roadmap.md): 永続的な継続性とメモリー
- [Personal World Model](spec/docs/saaa-personal-world-model-concept.md): 実体、関係、根拠、現在の理解
- [ロールルーティング実行契約](spec/docs/saaa-role-routing-execution-contract.md): Actor、recipe、予算、版管理
- [Adaptive Learning and Memory](spec/docs/saaa-adaptive-learning-memory-concept.html): Feedbackと選択的な記憶
- [Interface / Artifact Runtime](spec/docs/saaa-interface-artifact-runtime-concept.md): 対話的な出力と成果物
- [現在の改善計画](spec/docs/saaa-review-five-improvements-terra-plan.md): 接続課題と完了条件
- [設計文書一覧](spec/docs/README.html): 詳細計画、手順書、証跡

Conceptは目指す振る舞いを示します。実装計画と証跡では、完成した部分と残作業を区別しています。

## 開発への参加

不具合の再現手順、実装修正、文書や翻訳の改善、回帰テストを歓迎します。大きな振る舞いの変更では、問題と具体的な利用例をissueに記載してください。

[Contributing](CONTRIBUTING.md) · [Support](SUPPORT.md) · [Code of Conduct](CODE_OF_CONDUCT.md) · [Security](SECURITY.md)

## ライセンス

[MIT](LICENSE)。同梱する依存ライブラリとmodelにはそれぞれの条件があります。[第三者ライセンス](THIRD_PARTY_NOTICES.md)と[話者照合のライセンス](src-tauri/resources/voice/THIRD_PARTY_NOTICES.md)を参照してください。
