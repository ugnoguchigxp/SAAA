# LARM接続を完了するための依存作業

> この文書は当初の調査・実装時点の記録。製品API公開後の追加実装と現在の残課題は [製品接続の進捗](product-connection-progress.md) を参照。以下の「未接続」「作業範囲を確認中」は当時の状態であり、今回の追加実装には適用しない。

2026-09-13。確認先 `/srv/ai/apps/local-LLM-harness`、HEAD `631c15ad84eacb757770c92b21d647743cfeae6c`。ホスト上provision CLIとContext HTTP APIは存在するが、下記の製品用配送・計測・削除契約は確認できていない。以下は必要条件であり、既存API名や未確認endpointを表していない。

## LARM側に必要な契約

1. 認証済みprincipal/Allocation/release/runtime/tokenizer/chat templateと認定予算の取得。同principalでsource、View、generationを扱えること。lease期限と失効が照会可能なこと。
2. Macからの原文provision。SAAA発行incarnationを冪等キーにして、途中切断後もreceipt/sourceを照会できること。canonical digest、byteCount、tokenCount、tokenizerDigest、sourceHandleを返すこと。本文やcredentialを監査ログへ出さないこと。
3. 実際のmodel、messages、tools、chat template、包装を含むcanonical入力計測。推定文字数や別tokenizerの値で代用しないこと。
4. source本文・attestationの消去とabsence照会。登録前失敗・応答紛失による孤立sourceもincarnationからreconcileできること。
5. source由来およびbase messages由来のsnapshot依存を失効または確実に回避する操作と確認結果。Context登録DELETEとは分けること。
6. generationの取消・遠隔停止確認。HTTP切断の受領、materialization状態、遠隔推論停止、Task成功を混同しないこと。

公開方式と認可・冪等性・失効の仕様が確定した後、SAAAの `managed::Delivery` と `contract::Certification` を実装済み契約へ結線する。必要ならDeliveryをreceipt照会・remote stop対応へ拡張する。source登録HTTPの成功だけで上記を満たしたとしない。

## SAAA側の接続と受入

- 既存Agent Connectionに認証bindingを接続し、固定env token/手入力Certificationによる仮接続を製品の自動接続に置き換える。
- 認定済みProvider/Task経路から共通inferenceを使用する。外部coding subprocessなど、Contextを送れない経路を対応済みと扱わない。
- tool結果の正確な範囲が取得できない場合はSAAAの保守的な全候補原文依存を使う。その上限を超えた経路では状態patchを採用しない。引用根拠だけに忘却閉包を縮めない。
- source配送中、View作成後、生成中、保存直前のforget/lease失効/再起動をfixtureと実機で検証する。
- `personal-state:live` の実行先として、SAAAのsource保存→推論/抽出→永続化projection取得を行う製品用harnessを接続する。モデルへgoldを渡した直接応答を採点して代用しない。
- draft goldを人手確認し、snapshot OFF/ON、64件×3回、cold/warm各100件と資源計測を実施する。

LARMリポジトリを変更する作業範囲はユーザーへ確認中。SAAA内の型・模擬HTTP・runnerは接続先契約を捏造せず検証する。
