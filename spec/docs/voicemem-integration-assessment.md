# VoiceMemの配置・更新・SAAA連携に関する調査

調査日: 2026-09-13  
状態: 提案。連携実装・依存インストール・モデル起動は未実施。

## 結論

VoiceMem本体はSAAAにコピーせず、独立した上流依存として扱う。調査・開発用のcloneは `../VoiceMem` に置き、SAAA専用のAdapter、連携契約、検証、依存バージョンの指定をSAAA内で管理する構成を推奨する。

独立配置は可能だが、隣接ディレクトリで `git pull` するだけでSAAAが継続動作する保証はない。実行環境は検証済みcommitと依存一式に固定し、更新候補を別環境とデータコピーで検証してから切り替える。

コードの所有、実行プロセス、保存データの所有を分けることが要点である。VoiceMemのコードを独立管理しても、SAAAが使う個人記憶の保持・削除・バックアップについてはSAAAの製品責務として扱う。

## 調査対象と実施範囲

- 上流: [xzf-thu/VoiceMem](https://github.com/xzf-thu/VoiceMem)
- ローカルclone: `/Users/y.noguchi/Code/VoiceMem`
- 確認commit: `a450911fc8cbb44c46d810aace2f3288bad287e4`（2026-09-05）
- `pyproject.toml` のパッケージ版: `0.2.3`
- Gitタグ: `v0.0.1`、`v0.0.2`
- `v0.0.2` のcommit: `1b7cc9a0c2d63c37514f983ec6e53dde841e2d34`

上流コード、Git履歴、依存宣言、公開入口、保存処理、言語設定、テスト配置と、SAAAのREADME・package.json・保存先実装を静的に確認した。PyPI配布物とGit checkoutの内容一致、macOS上の依存解決、モデルの品質・速度は未検証。

READMEやパッケージメタデータには旧リポジトリ名の記述が残る。Gitタグとパッケージの版番号も一致していないため、初期評価ではcommit SHAを識別子に使う。

## 実装から確認したこと

| 確認事項 | 根拠と設計への影響 |
| --- | --- |
| Pythonライブラリとして利用できる | `voicemem/core.py` の `VoiceMem` が `ingest` / `search` / `flush` を公開する。デモアプリ全体を起動する必要はない |
| テキストだけで開始できる | `text_mode` があり、ASR・話者・音響処理はモードやフラグで分離できる。SAAAの確定済み発話を渡すPoCが可能 |
| 依存のインストールは軽量ではない | `pyproject.toml` の標準依存にtorch、torchaudio、torchvision、FunASR、モデル関連、音声I/O、Web関連が入る。実行時に音声機能を止めても依存宣言は軽くならない |
| 依存固定が不足する | 多くの依存がバージョン未指定。追跡ファイルにlockfileを確認できなかった。SAAA側で対象環境の解決結果を固定する必要がある |
| 外部の保存先を指定できる | `memory_root` が指定可能。既定値は作業ディレクトリ配下なので、製品利用では絶対パスを必ず渡す |
| 保存対象はSQLiteだけではない | MemorySpaceはSQLite、JSONメタデータ、ローカルQdrantの `vectors/`、`multi_modal/` を持つ。バックアップ・移行はspace全体を対象にする |
| 一部スキーマ移行が実装されている | `leftbrain/cognitive_graph/store.py` などに追加列の移行処理がある。ただし全体の版付き移行・ダウングレード保証までは確認できない |
| 設定にプロセス全体への副作用がある | モデル設定はグローバル表、APIキーは環境変数へ反映される。言語もspaceから解決後にグローバル値へ設定される。分離したworkerが適する |
| 日本語を記憶言語として選べない | `lang.py` の `SUPPORTED` は `en` / `zh`。日本語入力を処理できないという断定ではないが、日本語保持・検索・感情分類の品質は別途検証が必要 |
| 簡易保存APIは永続ジョブではない | `Memory.remember()` はdaemon threadで実行し、例外を表示するのみ。プロセス終了時の未完了や再試行をSAAAが扱う必要がある |
| 直接の保存APIには結果がある | `Ingest` は既定で同期実行し、`memory_ids` などを返す。時刻、session、話者、応答も渡せる。ただし全保存先の原子的成功は保証とみなさない |
| 検索結果は構造化されている | `SearchResult` と `MemorySearchHit` にID、本文、score、metadataなどがある。文字列を直接プロンプトへ注入するより、Adapterで正規化しやすい |
| 完全な訂正・削除契約は未確認 | 下位storeには更新・削除処理があるが、公開Facadeには派生情報を一括して訂正・削除する明確な入口を確認できなかった |
| Web実装はデモとして扱うべき | FastAPIの画面・音声・space管理が存在するが、SAAA向けに版管理された独立Memory service契約としては確認できなかった |

根拠は調査commitの [core.py](https://github.com/xzf-thu/VoiceMem/blob/a450911fc8cbb44c46d810aace2f3288bad287e4/voicemem/core.py)、[memory_api.py](https://github.com/xzf-thu/VoiceMem/blob/a450911fc8cbb44c46d810aace2f3288bad287e4/voicemem/memory_api.py)、[orchestrator.py](https://github.com/xzf-thu/VoiceMem/blob/a450911fc8cbb44c46d810aace2f3288bad287e4/voicemem/orchestrator.py)、[space.py](https://github.com/xzf-thu/VoiceMem/blob/a450911fc8cbb44c46d810aace2f3288bad287e4/voicemem/utils/common/space.py)、[lang.py](https://github.com/xzf-thu/VoiceMem/blob/a450911fc8cbb44c46d810aace2f3288bad287e4/voicemem/lang.py)、[pyproject.toml](https://github.com/xzf-thu/VoiceMem/blob/a450911fc8cbb44c46d810aace2f3288bad287e4/pyproject.toml)を参照。

## 管理方式の比較

| 方式 | 更新への対応 | SAAA側の負担 | 判断 |
| --- | --- | --- | --- |
| SAAA内へソースをコピー | 上流差分を手作業で反映 | 本体の保守まで抱える | 現段階では非推奨 |
| Git subtree | 履歴付きで取り込めるが変更の衝突を解決する必要がある | 本体の取り込みと製品変更が混ざりやすい | 継続的な独自改造が必要になった場合の候補 |
| Git submodule | commitをSAAAに記録できる | clone手順・CI・ビルドでsubmodule管理が必要 | ソース同梱が必要なら選択肢。現時点では必須でない |
| 隣接cloneをeditable installで常用 | pullが実行内容を即変更する | 再現性・切り戻しが弱い | 実験専用に限定 |
| 独立上流＋固定成果物＋SAAA Adapter | 上流更新を候補として評価し、検証済み版へ切り替える | Adapterと契約を保守する | 推奨 |

配置場所だけで更新耐性は決まらない。SAAAが上流内部のDBやクラスへどれだけ依存するか、実行版を固定できるか、データ移行を検証できるかが重要である。

## 推奨構成

以下は提案配置であり、Adapterやlockfileはまだ作成していない。

```text
Code/
├── SAAA/
│   ├── services/personal-memory/     SAAA所有のPython worker / Adapter
│   ├── src-tauri/...                worker接続、ジョブ、設定、状態管理
│   ├── tests/...                    SAAAの連携契約と回帰検証
│   └── spec/docs/...                設計判断、採用commit、検証結果
└── VoiceMem/                       上流の調査・開発用clone

SAAAアプリデータ/
├── saaa.sqlite3                    SAAAだけが書く
└── personal-memory/voicemem/...     VoiceMem workerだけが扱うspace

実行環境/
└── 検証済み版ごとのvenv・wheel・依存lock・モデル参照
```

開発時のclone位置を、配布版アプリの実行要件にしない。実際の配布では固定wheelとPython環境を同梱するか、管理された外部workerを設定する。どちらが適するかはmacOSでの依存サイズ・起動時間・更新方式を測って決める。

SAAAの `saaa.sqlite3` はRust `SqliteWriter` の単一writer設計である。VoiceMemのテーブルを同じDBへ入れ、Pythonから直接書かせない。VoiceMem用spaceはSAAAアプリデータ配下に置けるが、別のDBと保存ディレクトリとして扱う。

## Adapterが担うこと

SAAA → ローカルworker → VoiceMem Python APIという境界を設ける。最初の単一端末PoCでは、余分な常駐サーバを必要としない親子プロセス通信が候補になる。stdioを使う場合は上流の標準出力ログをプロトコルと分離する。HTTPやMCP化は複数クライアント等の必要が出てから選べる。

SAAA所有の最小契約は、health/version、observe、recall、flushを出発点にする。observeにはSAAAのevent ID、ユーザー、session、実際の発話時刻、本文、必要に応じて応答を含める。recallは本文だけでなく、根拠IDと上流memory IDを返す設計にする。

SAAAは永続的な送信待ちと処理結果を管理する。簡易 `remember()` の投げっぱなしを使わず、同期 `ingest` の結果をworkerが返す。ただし、保存直後・応答直前に停止した場合には重複の可能性が残る。公開APIにevent IDの冪等キーがないため、送信済み台帳だけでexactly-onceを保証しない。結果照合・重複検出・必要なら上流への小さな拡張をPoCで検討する。

上流Facadeは汎用metadataを自由に渡す契約ではない。SAAA event IDと返却memory IDの対応はAdapter側で記録し、右脳の派生情報を含めて根拠が追えるかを確認する。完全な訂正・削除は別途実装可否を確認する採用条件である。

`Memory.inject()` は検索結果をsystem messageとして挿入するため、そのまま使わない。構造化検索結果をSAAAのContext Composerへ渡し、記憶を参照情報として扱う。

モデル、接続先、embeddingを明示設定する。OpenAI互換URLを受け取れることは確認できたが、抽出・分類・右脳・embeddingの全経路が指定したローカル構成で動くかは実通信で確かめる。SAAAの既存LARM接続と資格情報の寿命にどう合わせるかもAdapter側の設計対象である。

## 上流更新の取り込み手順

1. 採用中commit、依存lock、Python版、Adapter契約版、モデル・embeddingの識別子、設定を記録する。
2. 隣接cloneで上流の更新を取得し、API・保存形式・プロンプト・依存の差分を確認する。稼働環境は変更しない。
3. 候補commitから別の固定実行環境を作る。実行時に隣接cloneを直接importしない。
4. workerの書き込みを止め、space全体とSAAA側の対応台帳を整合した状態でバックアップする。新環境にはデータのコピーを渡す。
5. 旧データの読み込み、想起、追加保存、再起動、訂正・削除、根拠対応を検証する。日本語の検索品質も比較する。
6. 検証後にworkerとデータ世代を切り替える。新旧workerを同じspaceへ同時接続しない。
7. 不具合があれば旧環境と旧データ世代へ戻す。切り替え後の新着イベントは保持された送信記録から再処理する。削除や撤回の記録も再適用し、情報を復活させない。

embeddingは次元だけでなくモデル・revisionも固定する。同じ次元でもモデルが違えば既存ベクトルと混在させない。変更時は再索引または新spaceへの再構築を検証する。

以上により上流の進化を取り込む道は確保できる。ただし、破壊的変更やデータ移行の不備に対してはAdapter修正、再索引、更新見送りが必要になる。

## 最初のPoCで判断すること

最初はSAAAの確定したテキスト発話と応答だけを使う。未保存の会議内容や生音声は対象にせず、既存のASR・話者フィルタ・TTSを置き換えない。

採用判断を左右する検証は次のとおり。

- 日本語入力から好み・個人経験・時系列を保存し、日本語の質問で正しく想起できるか。英語記憶を介する場合の意味のずれも見る。
- 左脳の事実記憶と右脳の推測を分けて取得し、根拠を表示できるか。
- 指定ローカルモデル・embeddingで全経路が動き、応答を過度に遅らせないか。
- 保存失敗・途中停止・再送で情報欠落や重複を検出できるか。
- 訂正と削除を派生情報まで反映できるか。
- 旧データを候補版で扱え、旧環境へ戻せるか。

日本語対応や削除契約に本体改修が必要なら、まずAdapterや公開拡張点で解決可能かを調べる。不可なら小さな変更を上流へ提案するか、別リポジトリのforkでpatchを管理する。SAAAへ本体をコピーする判断は、それでも独立更新より自前保守の利点が大きい場合に限る。

## World Modelとの境界

VoiceMemには既にentity・relation・感情等のグラフがあるが、これはVoiceMemの記憶内部表現である。SAAAの現在状態の正本として直接利用すると、上流のスキーマ変更がWorld Modelの変更になる。

World ModelはAdapterが返す正規化された記憶と根拠を入力として使い、VoiceMem内部テーブルへ依存しない。これにより、VoiceMem更新とWorld Model設計をそれぞれ進められる。
