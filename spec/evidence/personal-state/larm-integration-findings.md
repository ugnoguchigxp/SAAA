# Personal State実接続を止めている契約・実装の指摘

作成日: 2026-09-13。LARM担当・SAAA担当へ個別issueとして渡すための指摘文。

確認対象はgnosis上の `/srv/ai/apps/local-LLM-harness`、HEAD `631c15ad84eacb757770c92b21d647743cfeae6c`。作業ツリーに変更があるため、HEADだけで実稼働版の認定を表さない。原本を読み取っただけで、実source配送・実モデル推論・遠隔消去は実行していない。

**確認できたのは「SAAAが利用する製品用の契約が未確定・未接続」という問題であり、「LARM内部に対応機能が一切存在しない」という断定ではない。** 既存の公開経路がある場合は、その仕様・対象版・認可条件・試験を提示して接続する。ない場合はLARM側の追加実装が必要になる。以下の項目名は要求する機能であり、未確認のendpoint名を提案仕様として固定しない。

## 1. [接続必須] Macから原文をprovisionし、応答紛失後に照合する契約が未確定

**確認事実**

LARMの `apps/daemon/src/context-source-cli.ts:54–61` は、ホスト上の絶対ファイルパスを受け取るprovision CLIで、API tokenからprincipalを導出する。`packages/backends/src/context-store.ts:289,324` に内部provision実装はある。SAAAがMac上の原文を同principalへ配送する製品用経路は確認できていない。SAAAの `src-tauri/src/memory/personal_state/managed.rs` は `UnavailableDelivery` を生成しており、provisionは必ず未設定エラーになる。

**発生条件・影響**

Personal StateをONにしても、原文が必要なViewを製品環境では構成できない。また、provision成功後に応答が切断された場合、handleが分からず遠隔複製を消去できない可能性が残る。

**必要な対応**

- LARM担当: 認証principalに隔離された配送・provision経路を提示または追加する。SAAA発行のincarnationを受け取り、同じ操作の再送・結果照会ができるようにする。
- 結果にはsourceHandle、sourceDigest、byteCount、tokenCount、tokenizerDigestと操作の状態を含める。token数はLARM側のcanonical計測に基づくこと。
- 同じincarnationで異なる本文を送った場合の拒否、冪等性の保持期間、期限切れ後の扱いを仕様化する。
- SAAA担当: Deliveryへ操作IDと結果照会を追加し、配送前に保存したoutboxから復旧できるようにする。現在のtraitはreceipt照会を持たず、SAAA側にも改修が必要。

**完了条件**

合成原文で「通常成功」「同内容再送」「異内容再送」「provision直後の応答切断」「SAAA再起動」「配送中のforget」を試験する。応答紛失後も同じincarnationから結果を取得でき、重複・孤立sourceが照合・削除できること。別principalから本文・receiptを取得できないこと。

## 2. [接続必須] View作成前のcanonical入力計測をSAAAから呼べない

**確認事実**

View要求には `baseInputTokens` が必要。LARM内部のmaterialization時の計測と、SAAAがView作成前に利用する計測契約は別である。SAAAの `managed::Delivery::measure` は現在 `personal-canonical-measurement-unconfigured` を返す。

**発生条件・影響**

messages、tools、状態の包装を含む実入力が上限内か確認できず、View要求を正しく組み立てられない。文字数換算やsource token数との単純和で代用すると、実際のchat templateとの差で上限を超える。

**必要な対応**

- LARM担当: 使用するmodel/release/runtime/tokenizer/chat templateに結び付いたcanonical計測の利用方法を提示または追加する。
- 計測対象にmessages、tools、必要な包装を含める。何が `baseInputTokens` に含まれるか、View sourceの包装との分担を明記する。
- 計測結果を対象request digestと認定版に結び、内容変更・release変更後に流用できない契約にする。
- SAAA担当: 製品の計測結果をDeliveryへ接続し、本文・tools変更時は再計測と新Viewを要求する。

**完了条件**

日本語、tool定義、空messages、複数source、予算境界、計測後のrequest変更、tokenizer/release変更を試験する。事前計測とmaterializationの関係が仕様どおりで、required入力の予算超過を明示的に拒否できること。

## 3. [接続必須] SAAA利用者とLARM principal/Allocationの対応を確定できない

**確認事実**

SAAAはローカルprincipalを保持する一方、LARM CLIはtokenからprincipalを導出する。SAAAの `contract::Certification` は保存済み値を照合する型であり、既存Agent Connectionからprincipal、Allocation lease、release認定を自動取得・更新する実装ではない。接続用tokenも現状は環境変数経由である。

**発生条件・影響**

ローカルIDとLARM認証主体を根拠なく同一視できない。lease更新やrelease切替後に、古い認定・別principalのsourceを使わないことも製品接続では未検証。

**必要な対応**

- LARM担当: 認証主体、利用可能なAllocation/runtime/release/capability、lease epoch・期限、tokenizer、認定入力・出力・byte予算の取得方法を確定する。
- SAAA担当: 既存Agent Connectionの認証結果からローカルownerとの対応を保存し、更新・失効時にregistration/View/generationを再照合する。任意のIDコピーや検証済みbooleanの手入力を認定の根拠にしない。
- 文書の容量例を現releaseの認定値として転記しない。20M source quotaとmodel入力上限を別に扱う。

**完了条件**

別principal、別Allocation、期限切れ、lease更新、release変更を投入し、古いView・認定・出力の再利用が拒否されること。再接続では正しい主体に結び直され、tokenが診断・証跡へ出ないこと。

## 4. [忘却必須] 登録DELETEとsource本文の消去を分けて完了確認できない

**確認事実**

LARMの `apps/daemon/src/context-controller.ts:483–528` は登録metadataを削除してready Viewをinvalidにする。機能OFF時は削除せずreturnする。内部source削除は `packages/backends/src/context-store.ts:469` の別処理であり、SAAAから利用する製品用消去・absence照会経路は未確定。SAAAの `Delivery::erase` も未実装。

**発生条件・影響**

Context DELETEが成功しても、原文・attestationが残る場合がある。登録前の失敗で作られたsourceは、context IDだけでは追跡できない。

**必要な対応**

- LARM担当: 同principalのsource/attestation消去と結果照会、incarnationからの孤立source照合を提示または追加する。機能OFF中の削除・再起動後の扱いも定義する。
- SAAA担当: 登録absence、source absence、snapshot安全性を別状態としてoutboxに保存する。すべて確認するまでcleanup完了にしない。

**完了条件**

Context OFF、DELETE再送、削除途中の切断、再起動、登録前失敗を試験する。登録DELETEの成功だけで完了にならず、残存するsource/attestationを検出して再処理できること。物理secure eraseの保証とは区別すること。

## 5. [忘却必須] base messages由来のsnapshotを失効・回避する契約が未確定

**確認事実**

登録削除はready Viewの無効化を行うが、その処理だけでbase messages由来のsnapshotまで安全になることは確認できていない。SAAAは `base_snapshot_safe` という値を照合するだけで、具体的な失効操作や回避手順を実行する製品Adapterを持たない。

**発生条件・影響**

忘れた原文がsource登録ではなくbase messagesにだけ含まれていた場合、登録を消しても影響snapshotが再利用される可能性を排除できない。

**必要な対応**

- LARM担当: generation/requestとsnapshotの依存関係を追跡し、対象snapshotの失効、または影響snapshotを使わない再構築方法を提供する。対象範囲・完了確認・再起動後の保証を仕様化する。
- SAAA担当: base-only generationも忘却対象に含め、確認できるまで該当するmanaged推論を再開しない。

**完了条件**

合成の固有文字列をbaseだけ／sourceだけ／両方へ入れたケースで、forget後に影響snapshotが再利用されないことを状態・識別子で検証する。再起動・snapshot ON/OFFも含める。回答に文字列が出なかったことだけを失効の証明にしない。

## 6. [受入必須] HTTP取消と遠隔generation停止を結び付けて確認できない

**確認事実**

SAAAはHTTP futureを取り消し、遅延出力を拒否する処理を持つ。一方、Context operationの成功はmaterializationの完了であり、遠隔推論停止やTask成功を表さない。SAAAの現在のDelivery traitには遠隔停止照会がない。

**発生条件・影響**

foreground復帰・forgetでローカル処理を止めても、LARM側の計算終了を確認できない。停止済みと誤認すると次のgenerationと重なり、停止待ちを維持すると再開条件が分からない。

**必要な対応**

- LARM担当: SAAAのgeneration/attemptに対応する取消・停止照会の利用方法を確定する。少なくとも「取消受付」「遠隔停止確認」「結果不明」を区別できること。
- SAAA担当: ローカル取消時刻、送信時刻、遠隔停止確認時刻を分けて記録し、停止未確認を成功扱いしない。再起動後も照合可能にする。

**完了条件**

生成中のforeground復帰・forget、取消応答紛失、遅延応答、SAAA再起動を試験する。遅延結果の保存・表示・tool副作用が拒否され、同時generation上限を守って再開できること。取消送信p95目標100msと遠隔停止時間を別々に測定する。

## SAAA側に残る作業も別issueとして管理する

上の6項目をLARM側だけの不具合として扱わない。SAAAにも以下が残っている。

| 指摘 | 担当 | 完了条件 |
| --- | --- | --- |
| 製品Delivery・認証Adapterが未接続 | SAAA | 上記契約を実装し、UnavailableDeliveryと手入力認定に依存せず接続できる |
| 全Provider・外部coding subprocessのbindingが未完了 | SAAA | 対応経路ごとにmanifest・出典・取消・出力検査を検証し、非対応経路は明示する |
| 有限runnerの製品SAAA harnessが未接続 | SAAA | 合成sourceの保存→実推論/抽出→永続化projection取得を自動実行する。モデル直接回答による自己採点で代用しない |
| 実機受入・goldレビューが未実施 | SAAA／レビュー担当 | 人手確認済みgoldで64件×3回、残るC1〜C15、snapshot OFF/ON、cold/warm各100件、ASR/TTS・資源測定の証跡を残す |

実装順は1〜3の契約固定・接続、4〜6の忘却・停止確認、全経路の接続、実機受入とする。実ユーザーsourceの送信を始める前に、合成データで認可・忘却・遅延出力拒否を確認する。
