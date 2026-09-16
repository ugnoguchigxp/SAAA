# LARM契約照合 — Personal State計画の再設計

確認日: 2026-09-13。SAAAの計画修正に必要な原本・コードを、既存SSH接続で読み取りのみ確認した。モデル起動、設定変更、source provision、generation、外部タスクへの送信は行っていない。

## 対象版

- SAAA HEAD: `cc8c66c`。`src-tauri/src/providers/larm_voice/decision.rs`、`crates/larm-session/src/contract.rs`、`src-tauri/src/providers/stream/larm.rs`、memory control_planeと既存計画を確認。
- LARM root: gnosis上の `/srv/ai/apps/local-LLM-harness`。
- LARM HEAD: `631c15ad84eacb757770c92b21d647743cfeae6c`。
- 下記役割境界文書は作業ツリーで未追跡。他の照合対象ファイルにはstatus差分なし。役割方針をこのcommitに含まれる仕様と誤記しない。

| ファイル | SHA-256 |
| --- | --- |
| specs/inference-first-host-responsibility-boundary.html | 511f3e20e50cbe22cbf1e4eae7760c5269c1afeac86afda0ad434ed0be621d59 |
| packages/core/src/context.ts | 29bfcd84214c188d61510720a314b6cd82dce6594c16785bc8fc25bc8e2d4feb |
| apps/daemon/src/context-controller.ts | 06b9d9254ea75bcccaccb6ef5f31341aa6f9027091813ea20316f1bc8edf3bab |

## 確認事実と計画への反映

行番号は確認時点の原本を指す。遠隔ファイルをSAAAのローカルファイルとしてリンクしない。

| 根拠 | 確認内容 | 設計判断 |
| --- | --- | --- |
| README.md:166〜198 | 20Mはsource集合。modelだけではsnapshotを使わない。明示Allocation・source登録・View・3 headerが必要。Viewはone-shot | 20M連続Context案を撤回。source/registration/View/generationを分離 |
| deploy/local-node/README.md:202〜229 | 本文はContext APIへinlineせず同principalへprovision。canonical attestation。20M quota、262,144 native、32,768出力予約、4,096余白、225,280入力 | Macからhostへの配送をP1-00依存とする。3つの容量を区別 |
| packages/core/src/context.ts:93〜200 | classification、source版/digest、required/utility、baseInputTokens、1〜512 items、binding、omitted、operation state | SAAAに登録台帳/GenerationManifest、required検査、用途別認可を置く |
| packages/core/src/context.ts:266以降 | requiredはcanonical順、optionalはutility順。requiredの予算超過は失敗 | SAAAは任意順序を要求せず、必須状態をutilityで落とさない |
| apps/daemon/src/context-controller.ts:483〜528 | DELETEは同principal/context IDの全versionを除去。ready Viewをinvalidにする。active generation停止・物理source削除はこの関数にない | 登録解除と忘却を分離。SAAAがHTTP cancel・出力fenceを担当 |
| apps/daemon/src/context-controller.ts:530〜707 | Allocation/release/認定照合、required欠落拒否、View期限、idempotency | 新generationには有効な新Viewを作成。登録成功とconsume成功は別 |
| apps/daemon/src/context-controller.ts:712〜1019 | View消費確認、snapshot適合確認、source読み取り、system内data block、canonical実入力計測。Context operation成功はmaterialization完了時 | task成功と混同しない。base/source token単純和では上限保証不可 |
| packages/backends/src/context-store.ts:469以降 | source本文とattestationをunlinkする別delete実装 | 公開cleanup配送経路は別途確認。Context DELETE成功で完全消去と表示しない |
| specs/inference-first-host-responsibility-boundary.html:311〜383 | tacticalに事実回答・約束・tool・Memory/World更新を禁止。semantic readinessとmixed認定 | 2Bは構造化候補/定型のみ。27Bが最終回答。無条件並行generationを除外 |
| 同文書:780〜800 | active requestはnon-preemptive、SAAA HTTP cancelが必要。activityは予約ではない。自動排他未保証 | workerの実HTTP取消とSAAA内schedulerをP1へ含める |
| SAAA decision.rs冒頭・classify_shadow | 2Bの分類はshadow-onlyで回答を変更しない | P1で自由会話やproduction routingを暗黙に有効化しない |

## 今回確認していない事項

- Mac→LARMの製品用source配送・provision・cleanup経路の利用可否。
- Agent Connection claimからContext用Allocation/capabilityへの具体的接続と全UI経路の対応。
- 実releaseのsemantic readiness、mixed性能、snapshot hit率、dynamic base/prefixの性能。
- 本番データでの抽出精度、遅延、forgetの遠隔停止時間、物理secure erase。

これらはP1-00または実機受入の成果物とする。未確認のAPIを創作せず、実装済み・認定済みと報告しない。

## 改訂成果物

- [全体設計・ロードマップ](personal-state-architecture-roadmap.md)
- [P1実装計画](personal-state-phase-1-plan.md)

概念文書の20M/2B説明も同時に整合させる。旧計画のレビュー完了記録は、LARM前提を検証できていなかったため撤回・更新した。

## 再レビュー記録（2026-09-13）

LARM HEADと上記3ファイルのdigestを再照合し、前回確認時と同一であることを確認した。controllerのdelete（488行）ではContext機能OFF時に削除処理を行わずreturnするため、HTTP成功をcleanup完了と扱わない契約を追加した。SAAAのcontext_window.rsは最大400件・長文4,000文字の先頭/末尾抽出を行うため、export/抽出に用いる正本loaderを分離する計画へ修正した。

| レビューで見つかった不足 | 修正先 | 検証との対応 |
| --- | --- | --- |
| 同じTask内のgeneration・再試行・候補採用の識別不足 | P1第4・5・9節のgeneration/attempt、採用fence | C2・C7・C14 |
| 忘却後に遅い本文が保存される余地、取消と送出の競合 | P1第5・11節の保存入口とtombstone境界、cache無効化 | C5・C6・C12・C14 |
| 登録解除後のreplay、pinと解除の競合、OFF中DELETE | P1第8・11節のincarnation、状態照合、pending | C8・C12・C13 |
| 引用範囲だけを分類・忘却の依存とする不足 | P1第4・9節の支持根拠と全入力依存の分離 | C2・C3・C4・C5 |
| 初期履歴/長文/版更新と時刻失効の未定義動作 | P1第4・5節のcoverage、分割最終化、loader、読取時刻 | C4・C15 |
| 品質指標の分母と失敗要求の性能集計が曖昧 | P1第14節のgold照合・未抽出率・正常系失敗gate | Q1・P1 |

2回の修正後、責務・各状態遷移・実装担当・受入行列の対応を再点検した。レビュー完了の対象は文書上の整合と異常時動作の定義である。source配送、snapshot失効の公開経路、実機取消・性能・日本語精度はP1-00/P1-07で確認する未検証事項として維持し、文書修正だけで実装可能性の実証や性能認定を完了したとは扱わない。
